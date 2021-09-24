//! For high-level documentation, see the [Readme](https://github.com/typesanitizer/concurrent-interner).

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;

use std::fmt::Debug;
use std::hash::{BuildHasher, Hash, Hasher};
use std::mem::ManuallyDrop;
use std::num::NonZeroU8;
use std::sync::Mutex;

#[cfg(feature = "_serial")]
pub mod serial;

//----------------------------------------------------------------------------
// Public API - ConcurrentInterner, type definition
//
// See [REF NOTE: interner-design] for higher-level discussion.

/// A thread-safe string interner.
///
/// The APIs are based on the assumption that you will create one
/// [`ConcurrentInterner`], intern some strings to get some [`IStr`]s,
/// and then potentially try to obtain the strings back from the same interner.
/// Using the [`IStr`] from one interner to obtain the data from another
/// interner may lead to incorrect results or a panic. It will not lead
/// to memory unsafety.
///
/// See also: [`ConcurrentInternerMember`] and [`ConcurrentInterner::get_member()`].
///
/// The `RS` generic parameter should be substituted with a `RandomState` type.
/// For example, in the context of a compiler, you could use:
/// - [`ahash::RandomState`](https://docs.rs/ahash/latest/ahash/struct.RandomState.html).
/// - [`rustc_hash::FxHasher`](https://docs.rs/rustc-hash/latest/rustc_hash/struct.FxHasher.html)
///   wrapped by a [`BuildHasherDefault`](https://doc.rust-lang.org/std/hash/struct.BuildHasherDefault.html).
///
/// In some early testing, FxHash sometimes turns out to be a smidge faster
/// for a workload involving code, but honestly it is hard to tell the difference.
pub struct ConcurrentInterner<RS: Send + Sync + Clone + BuildHasher> {
    /// Thread-safe hash-table for mapping a small string -> IStr.
    indices: ConcurrentInternerMap<RS>,

    /// Storage arenas for different interner members.
    ///
    /// The number of arenas is fixed at initialization time.
    storage: Mutex<ConcurrentInternerStorage>,
}

//------------------------------------------------------------------
// Helper type definition

type ConcurrentInternerMap<RS> = DashMap<SSOStringRef, IStr, RS>;

//------------------------------------------------------------------
// Trait implementations

impl<RS: Send + Sync + Clone + BuildHasher> std::fmt::Debug for ConcurrentInterner<RS> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg_map = f.debug_map();
        for kvref in self.indices.iter() {
            let key_bytes = unsafe { kvref.key().as_bytes() };
            let key_str = std::str::from_utf8(key_bytes).expect("invariant violation: invalid utf-8 in storage");
            dbg_map.entry(&key_str, kvref.value());
        }
        dbg_map.finish()
    }
}

//------------------------------------------------------------------
// Constants

impl<RS: Send + Sync + Clone + BuildHasher> ConcurrentInterner<RS> {
    const LOCK_ACQUIRE_ERROR_MSG: &'static str = "Unable to acquire lock for interner storage.";
}

//------------------------------------------------------------------
// Construction API

impl<RS: Send + Sync + Clone + BuildHasher> ConcurrentInterner<RS> {
    /// Create an interner with one or more arenas.
    ///
    /// In most cases, the number of arenas will be equal to the number of
    /// threads simultaneously using the [`ConcurrentInterner`].
    /// You should perform some benchmarking for the right `arena_count`
    /// value, instead of directly setting it to the number of CPU cores,
    /// since the backing [`DashMap`] does not scale very well under
    /// high contention.
    /// (See [Performance numbers](https://github.com/typesanitizer/concurrent-interner/blob/main/docs/Performance.md)).
    ///
    /// If you're not sure what `random_state` to plug in,
    /// [`Default::default()`] is a reasonable choice.
    pub fn new(arena_count: NonZeroU8, random_state: RS) -> ConcurrentInterner<RS> {
        // TODO: Expose another constructor which allows passing in a shard amount,
        // which can be used by ourselves and clients for cross-interpretation
        // under MIRI. By default, this does I/O by using the open64 syscall
        // (to read the number of CPUs), which MIRI doesn't support.
        ConcurrentInterner {
            indices: ConcurrentInternerMap::with_capacity_and_hasher(64, random_state),
            storage: Mutex::new(ConcurrentInternerStorage {
                handed_out_count: 0,
                boxes: (0..arena_count.get())
                    .map(|i| Box::new(ConcurrentInternerMemberStorage::new(i)))
                    .collect(),
            }),
        }
    }
}

//------------------------------------------------------------------
// Query API

impl<RS: Send + Sync + Clone + BuildHasher> ConcurrentInterner<RS> {
    /// Get a member of the interner, which can be used to intern strings.
    ///
    /// NOTE: This method acquires a lock, so you don't want to call it
    /// willy-nilly (such as for interning a single string).
    /// Instead, call this when starting to do interning work
    /// from each thread.
    pub fn get_member(&self) -> ConcurrentInternerMember<'_, RS> {
        let mut lock = self.storage.lock().expect(Self::LOCK_ACQUIRE_ERROR_MSG);
        let storage = lock
            .boxes
            .pop()
            .expect("All ConcurrentInternerMembers are taken!");
        lock.handed_out_count += 1;
        ConcurrentInternerMember {
            indices: &self.indices,
            storage: ManuallyDrop::new(storage),
            owner: self,
        }
    }

    /// Read a string from a [`ConcurrentInterner`].
    ///
    /// This API is primarily present for convenience and debugging.
    ///
    /// If you want to retrieve a large number of strings, you can call
    /// [`ConcurrentInterner::freeze`]
    /// followed by [`FrozenInterner::get_str`] to directly copy the
    /// data out, without an intermediate heap allocation.
    pub fn get_string(&self, istr: IStr) -> String {
        debug_assert!(self.is_organized(), "Forgot to call reorganize?");
        let storage = &self
            .storage
            .lock()
            .expect(Self::LOCK_ACQUIRE_ERROR_MSG)
            .boxes[istr.arena_id() as usize];
        storage.get_str(istr).to_string()
    }
}

//------------------------------------------------------------------
// Deconstruction API

impl<RS: Send + Sync + Clone + BuildHasher> ConcurrentInterner<RS> {
    /// Make an interner read-only, allowing for faster string accesses.
    pub fn freeze(self) -> FrozenInterner<RS> {
        debug_assert!(self.is_organized(), "Forgot to call reorganize?");
        FrozenInterner {
            indices: self.indices,
            frozen_storage: self
                .storage
                .into_inner()
                .expect(Self::LOCK_ACQUIRE_ERROR_MSG),
        }
    }
}

//------------------------------------------------------------------
// Private API

impl<RS: Send + Sync + Clone + BuildHasher> ConcurrentInterner<RS> {
    /// Helper method for checking that the interner is correctly organized.
    fn is_organized(&self) -> bool {
        let storage_vec = self.storage.lock().expect(Self::LOCK_ACQUIRE_ERROR_MSG);
        return storage_vec
            .boxes
            .iter()
            .enumerate()
            .all(|(i, member)| (member.arena_id as usize) == i);
    }
}

//----------------------------------------------------------------------------
// Public API - ConcurrentInternerMember type definition

/// A member for an interner, used to intern strings into the interner
/// from a thread without locking.
pub struct ConcurrentInternerMember<'i, RS: Send + Sync + Clone + BuildHasher> {
    indices: &'i ConcurrentInternerMap<RS>,
    storage: ManuallyDrop<Box<ConcurrentInternerMemberStorage>>,
    owner: &'i ConcurrentInterner<RS>,
}

//------------------------------------------------------------------
// Trait implementations

impl<RS: Send + Sync + Clone + BuildHasher> Drop for ConcurrentInternerMember<'_, RS> {
    fn drop(&mut self) {
        let mut guard = self
            .owner
            .storage
            .lock()
            .expect(ConcurrentInterner::<RS>::LOCK_ACQUIRE_ERROR_MSG);
        // See discussion in bumpalo-herd's comments.
        // https://docs.rs/bumpalo-herd/0.1.1/src/bumpalo_herd/lib.rs.html#249
        let storage = unsafe { ManuallyDrop::take(&mut self.storage) };
        guard.boxes.push(storage);
        guard.handed_out_count -= 1;
        if guard.handed_out_count == 0 {
            guard.boxes.sort_by_key(|s| s.arena_id);
        }
    }
}

//------------------------------------------------------------------
// Constants

impl<'i, RS: Send + Sync + Clone + BuildHasher> ConcurrentInternerMember<'i, RS> {
    const STORAGE_CHUNK_SIZE: usize = ConcurrentInternerMemberStorage::STORAGE_CHUNK_SIZE;

    const CHUNK_SLICE_ERROR_MSG: &'static str =
        ConcurrentInternerMemberStorage::CHUNK_SLICE_ERROR_MSG;
}

//------------------------------------------------------------------
// Modification API

impl<'i, RS: Send + Sync + Clone + BuildHasher> ConcurrentInternerMember<'i, RS> {
    /// Intern a string slice, returning a unique [`IStr`].
    ///
    /// If multiple interners attempt to simultaneously intern the same string,
    /// it is guaranteed that the returned [`IStr`] values will be equal.
    pub fn intern(&mut self, s: &str) -> IStr {
        let sref = SSOStringRef::new(s);
        if let Some(istr) = self.indices.get(&sref) {
            return istr.value().clone();
        }

        let istr: IStr;
        let potential_new_key: SSOStringRef;
        let storage = self.storage.as_mut();
        let composite_index: CompositeIndex;
        if s.len() <= storage.bytes_left {
            let chunk_index = storage.chunks.len() - 1;
            let start = Self::STORAGE_CHUNK_SIZE - storage.bytes_left;
            let range = start..start + s.len();
            // Remove .clone() once https://github.com/rust-lang/rfcs/issues/2848 is fixed.
            storage.chunks[chunk_index][range.clone()].copy_from_slice(s.as_bytes());
            storage.bytes_left -= s.len();
            // SAFETY: This is safe because (1) only we have mutable
            // access to the storage and (2) we literally just copied
            // a valid UTF-8 slice to that range.
            potential_new_key = unsafe {
                SSOStringRef::new_unchecked(
                    &storage.chunks[chunk_index]
                        .get(range.clone())
                        .expect(Self::CHUNK_SLICE_ERROR_MSG),
                )
            };
            debug_assert!(start <= u16::MAX as usize);
            composite_index =
                CompositeIndex::new_chunk_index(chunk_index, start as u16, s.len() as u16);
        } else if s.len() <= Self::STORAGE_CHUNK_SIZE {
            let chunk_index = storage.chunks.len();
            storage.chunks.push(Box::new(
                // Cannot use Self::STORAGE_CHUNK_SIZE due to
                // https://github.com/rust-lang/rust/issues/89236
                [0; ConcurrentInternerMemberStorage::STORAGE_CHUNK_SIZE],
            ));
            let range = 0..s.len();
            // Remove .clone() once https://github.com/rust-lang/rfcs/issues/2848 is fixed.
            storage.chunks[chunk_index][range.clone()].copy_from_slice(s.as_bytes());
            storage.bytes_left = Self::STORAGE_CHUNK_SIZE - s.len();
            // SAFETY: This is safe because (1) only we have mutable
            // access to the storage and (2) we literally just copied
            // a valid UTF-8 slice to that range.
            potential_new_key = unsafe {
                SSOStringRef::new_unchecked(
                    &storage.chunks[chunk_index]
                        .get(range)
                        .expect(Self::CHUNK_SLICE_ERROR_MSG),
                )
            };
            composite_index = CompositeIndex::new_chunk_index(chunk_index, 0, s.len() as u16);
        } else {
            let huge_index = storage.huge_strings.len();
            storage.huge_strings.push(s.to_string());
            potential_new_key = SSOStringRef::new(storage.huge_strings[huge_index].as_str());
            composite_index = CompositeIndex::new_huge_index(huge_index);
        }
        let arena_local_index = storage.alloc_indices.len();
        storage.alloc_indices.push(composite_index);
        istr = IStr::new(storage.arena_id, arena_local_index);

        // It is possible that in the meantime, some other thread allocated
        // the same string into its local arena, entered it into the table,
        // and returned the corresponding IStrIndex. If that happened, we
        // need to use the value it returned to be able to compare IStrs
        // correctly, and ignore our freshly allocated slice.

        match self.indices.entry(potential_new_key) {
            // TODO: Make some measurements on memory wastage.
            Entry::Occupied(o) => {
                self.storage.wasted_bytes += s.len();
                return *o.get();
            }
            Entry::Vacant(v) => {
                v.insert(istr);
                return istr;
            }
        }
    }
}

//----------------------------------------------------------------------------
// Public API - IStr type definition

/// An opaque interned string that is guaranteed to be 32 bits in size.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct IStr {
    raw: u32,
}

//------------------------------------------------------------------
// Trait implementations

impl std::fmt::Debug for IStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IStr")
            .field("arena_id", &self.arena_id())
            .field("arena_local_id", &self.arena_local_id())
            .finish()
    }
}

//------------------------------------------------------------------
// Private API

impl IStr {
    fn new(arena_id: u8, arena_local_index: usize) -> IStr {
        std::debug_assert!(arena_local_index < (2 << 24));
        IStr {
            raw: ((arena_id as u32) << 24) | (arena_local_index as u32),
        }
    }

    const fn arena_id(&self) -> u8 {
        (self.raw >> 24) as u8
    }

    const fn arena_local_id(&self) -> u32 {
        (self.raw << 8) >> 8
    }
}

//----------------------------------------------------------------------------
// Internal API - ConcurrentInternerStorage

struct ConcurrentInternerStorage {
    /// The number of storage boxes that have been vended out.
    handed_out_count: u8,
    boxes: Vec<Box<ConcurrentInternerMemberStorage>>,
}

//----------------------------------------------------------------------------
// Internal API - InternerMemberStorage

/// Arena type for allocating storage for strings in an interner.
struct ConcurrentInternerMemberStorage {
    /// The original index of this storage in the parent Interner.
    arena_id: u8,

    /// Number of bytes left in the current chunk.
    bytes_left: usize,

    alloc_indices: Vec<CompositeIndex>,

    /// Storage for strings using fixed-size chunks.
    ///
    /// Compared to using an increasing size for chunks, this means
    /// the amount of wasted (allocated but unused) memory is bounded.
    chunks: Vec<Box<[u8; Self::STORAGE_CHUNK_SIZE]>>,

    /// Standalone storage for strings bigger than STORAGE_CHUNK_SIZE.
    huge_strings: Vec<String>,

    /// Number of bytes wasted due to lost races.
    ///
    /// See [REF NOTE: interner-overallocate].
    wasted_bytes: usize,
}

//------------------------------------------------------------------
// Constants

impl ConcurrentInternerMemberStorage {
    // IDEA: Is there a crate which exposes page size constants?
    // I found https://crates.io/crates/page_size, but that is dynamic.

    // IDEA: Is there a way we can measure experimentally the malloc overhead
    // across different malloc implementations? It would be nice to
    // have some tests for that.

    // Save 24 bytes for the malloc implementation on Darwin.
    // https://github.com/apple/swift/commit/64c670745a5bbf4758aa98e96996a5cf53dac344
    #[cfg(all(target_arch = "aarch64", any(target_os = "macos", target_os = "ios")))]
    const STORAGE_CHUNK_SIZE: usize = 16384 - 24;

    // Might as well save 24 bytes elsewhere as well. 🤷🏽
    #[cfg(not(all(target_arch = "aarch64", any(target_os = "macos", target_os = "ios"))))]
    const STORAGE_CHUNK_SIZE: usize = 4192 - 24;

    const CHUNK_SLICE_ERROR_MSG: &'static str = "Trying to slice chunk with out-of-bounds indices.";
}

//------------------------------------------------------------------
// Construction API

impl ConcurrentInternerMemberStorage {
    fn new(original_index: u8) -> ConcurrentInternerMemberStorage {
        ConcurrentInternerMemberStorage {
            arena_id: original_index,
            bytes_left: Self::STORAGE_CHUNK_SIZE,
            alloc_indices: vec![],
            chunks: vec![Box::new([0; Self::STORAGE_CHUNK_SIZE])],
            huge_strings: vec![],
            wasted_bytes: 0,
        }
    }
}

//------------------------------------------------------------------
// Query API

impl ConcurrentInternerMemberStorage {
    fn get_str(&self, istr: IStr) -> &str {
        let alloc_id = istr.arena_local_id();
        let composite_index = self.alloc_indices[alloc_id as usize];
        if let Some(huge_index) = composite_index.huge_index() {
            return &self.huge_strings[huge_index as usize];
        }
        // Technically, it is possible that we got the interned string from
        // a different interner entirely, are trying to use that here,
        // which could lead us to breaking some bytes the wrong way,
        // but I'm ignoring that for now.
        let (chunk_index, chunk_start_offset, len) = composite_index
            .chunk_info()
            .expect("Unexpected huge string index instead of chunk index.");
        let (chunk_index, chunk_start_offset, len) = (
            chunk_index as usize,
            chunk_start_offset as usize,
            len as usize,
        );
        let bytes = self.chunks[chunk_index]
            .get(chunk_start_offset..(chunk_start_offset + len))
            .expect(Self::CHUNK_SLICE_ERROR_MSG);
        // TODO: Profile if UTF-8 checking here makes a meaningful difference.
        std::str::from_utf8(bytes).expect("Expected valid UTF-8")
    }
}

//----------------------------------------------------------------------------
// Public API - FrozenInterner type definition

/// Read-only version of [`ConcurrentInterner`].
///
/// Allows read access from multiple threads without synchronization
/// in exchange for not allowing any writes.
pub struct FrozenInterner<RS: Send + Sync + Clone + BuildHasher> {
    indices: ConcurrentInternerMap<RS>,
    frozen_storage: ConcurrentInternerStorage,
}

//------------------------------------------------------------------
// Trait implementations

impl<RS: Send + Sync + Clone + BuildHasher> From<ConcurrentInterner<RS>> for FrozenInterner<RS> {
    /// Identical to [`ConcurrentInterner::freeze`].
    fn from(interner: ConcurrentInterner<RS>) -> FrozenInterner<RS> {
        interner.freeze()
    }
}

impl<RS: Send + Sync + Clone + BuildHasher> std::fmt::Debug for FrozenInterner<RS> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg_map = f.debug_map();
        for kvref in self.indices.iter() {
            let key_bytes = unsafe { kvref.key().as_bytes() };
            let key_str = std::str::from_utf8(key_bytes).expect("invariant violation: invalid utf-8 in storage");
            dbg_map.entry(&key_str, kvref.value());
        }
        dbg_map.finish()
    }
}

//------------------------------------------------------------------
// Query API

impl<RS: Send + Sync + Clone + BuildHasher> FrozenInterner<RS> {
    pub fn get_str(&self, istr: IStr) -> &str {
        self.frozen_storage.boxes[istr.arena_id() as usize].get_str(istr)
    }
}

//----------------------------------------------------------------------------
// Internal API - ArenaLocalIndex type definition

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
struct ArenaLocalIndex {
    raw: u32,
}

//------------------------------------------------------------------
// Construction API

impl ArenaLocalIndex {
    #[inline]
    fn new_chunk_index(index: u32) -> ArenaLocalIndex {
        debug_assert!(index >> 31 == 0);
        ArenaLocalIndex { raw: index }
    }

    #[inline]
    fn new_huge_index(index: u32) -> ArenaLocalIndex {
        debug_assert!(index >> 31 == 0);
        ArenaLocalIndex {
            raw: (1 << 31) | index,
        }
    }
}

//------------------------------------------------------------------
// Query API

impl ArenaLocalIndex {
    #[inline]
    const fn get_index_unchecked(&self) -> u32 {
        (self.raw << 1) >> 1
    }

    #[inline]
    const fn is_huge(&self) -> bool {
        (self.raw >> 31) == 1
    }

    #[inline]
    const fn chunk_index(&self) -> Option<u32> {
        if self.is_huge() {
            None
        } else {
            Some(self.get_index_unchecked())
        }
    }

    #[inline]
    const fn huge_index(&self) -> Option<u32> {
        if self.is_huge() {
            Some(self.get_index_unchecked())
        } else {
            None
        }
    }
}

//----------------------------------------------------------------------------
// Internal API - CompositeIndex type definition

// NOTE: We have a limit of 2^24 composite indices. How much string
// storage does that correspond to?
//
// If we had all 3-byte u8 sequences, we'd need 2^24 composite indices.
// So we can store at least 2^24 * 3 bytes = 48 MiB worth of strings.
// However, given that not all possible 3-byte u8 sequences are valid
// strings, plus the fact that people's code commonly reuses
// identifiers longer than 3 bytes, I think it is safe to assume
// that 24-bits for composite indices are enough.
//
// For context, at the time of writing, the total size of .cpp files
// in the entire llvm-project is 81 MiB.

#[derive(Copy, Clone, Debug)]
struct CompositeIndex {
    arena_local_index: ArenaLocalIndex,
    chunk_start_offset: u16,
    len: u16,
}

//------------------------------------------------------------------
// Construction API

impl CompositeIndex {
    #[inline]
    fn new_huge_index(index: usize) -> CompositeIndex {
        debug_assert!(index <= u32::MAX as usize);
        CompositeIndex {
            arena_local_index: ArenaLocalIndex::new_huge_index(index as u32),
            chunk_start_offset: u16::max_value(),
            len: u16::max_value(),
        } // Usage of u16::max_value() will trigger a panic on index confusion.
    }

    #[inline]
    fn new_chunk_index(index: usize, chunk_start_offset: u16, len: u16) -> CompositeIndex {
        debug_assert!(index <= u32::MAX as usize);
        debug_assert!(
            (chunk_start_offset as usize) < ConcurrentInternerMemberStorage::STORAGE_CHUNK_SIZE
        );
        debug_assert!(
            chunk_start_offset as usize + (len as usize)
                <= ConcurrentInternerMemberStorage::STORAGE_CHUNK_SIZE
        );
        CompositeIndex {
            arena_local_index: ArenaLocalIndex::new_chunk_index(index as u32),
            chunk_start_offset,
            len,
        }
    }
}

//------------------------------------------------------------------
// Query API

impl CompositeIndex {
    #[inline]
    const fn huge_index(&self) -> Option<u32> {
        self.arena_local_index.huge_index()
    }

    #[inline]
    const fn chunk_info(&self) -> Option<(u32, u16, u16)> {
        // Option::map is not const as of current Rust version. 😔
        if let Some(chunk_index) = self.arena_local_index.chunk_index() {
            Some((chunk_index, self.chunk_start_offset, self.len))
        } else {
            None
        }
    }
}

//----------------------------------------------------------------------------
// Internal API - Small-size optimized &str.

// TODO: Benchmark if SSOStringRef is actually faster than &str.
//
// This was originally implemented so that small strings would have
// fast paths for comparison and hashing, but yeah, it leads to a bunch
// of additional branching which may confuse a branch predictor.

struct SSOStringRef {
    inner: SSOStringRefInner,
}

//------------------------------------------------------------------
// Helper type definition

/// Implementation for an SSOStringRef.
///
/// The possible states are as follows:
///
/// 1. Empty string: Union is fully zeroed out.
/// 2. Inline data:  First byte is non-zero and string is either
///                  null-terminated or takes the full 2 words.
///                  The string does not contain nulls.
/// 3. Non-inline data: First byte is zero, first word is length (upto
///                     shifts), second is ptr.
#[repr(C)]
union SSOStringRefInner {
    sref: ByteSlice,
    bytes: [u8; ByteSlice::SIZE_IN_BYTES],
}

//------------------------------------------------------------------
// Trait implementations

// SAFETY: This unsafe implementation is OK because any pointer
// inside is only dereferenced when the storage is available.

unsafe impl Send for SSOStringRef {}
unsafe impl Sync for SSOStringRef {}

impl PartialEq for SSOStringRef {
    fn eq(&self, other: &SSOStringRef) -> bool {
        // NOTE: Unlike C++, unions in Rust have no notion of "active field".
        // So the punning between bytes and sref is ok.
        // See https://doc.rust-lang.org/reference/items/unions.html
        if unsafe { self.inner.sref.len != other.inner.sref.len } {
            return false;
        }
        // Lengths are now bitwise equal.
        unsafe {
            if self.inner.sref.len == 0 {
                // Both strings are empty.
                return true;
            } else if self.has_inline_data() {
                // If the first string has inline data, then so does the
                // second one, because otherwise their first bytes would've
                // been different, so the len values would've been different.
                //
                // Even though we don't have pointers, instead of doing
                // byte-wise comparison, we can treat the rest of the data as
                // pointers and compare the two pointers.
                std::debug_assert!(other.has_inline_data());
                return self.inner.sref.ptr == other.inner.sref.ptr;
            } else {
                // Now we really have two pointers on our hands
                return (self.inner.sref.ptr == other.inner.sref.ptr)
                    || (self.inner.sref.as_bytes() == other.inner.sref.as_bytes());
            }
        }
    }
}

impl Eq for SSOStringRef {}

impl Hash for SSOStringRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (unsafe { self.as_bytes() }).hash(state);
    }
}

//------------------------------------------------------------------
// Construction API

impl SSOStringRef {
    /// Construct an SSOStringRef from a byte slice.
    ///
    /// NOTE: The bytes are assumed to be valid UTF-8 but this is not checked.
    unsafe fn new_unchecked(s: &[u8]) -> SSOStringRef {
        assert!(
            s.len() <= ByteSlice::MAX_LEN,
            "Trying to intern string bigger than 16 MiB on 32-bit?"
        );
        if s.len() == 0 {
            SSOStringRef {
                inner: SSOStringRefInner {
                    bytes: [0; ByteSlice::SIZE_IN_BYTES],
                },
            }
        } else if s.len() <= ByteSlice::SIZE_IN_BYTES && s.iter().all(|&c| c != 0) {
            let mut sso_stringref = SSOStringRef {
                inner: SSOStringRefInner {
                    bytes: [0; ByteSlice::SIZE_IN_BYTES],
                },
            };
            for (i, &c) in s.iter().enumerate() {
                sso_stringref.inner.bytes[i] = c;
            }
            sso_stringref
        } else {
            SSOStringRef {
                inner: SSOStringRefInner {
                    sref: ByteSlice::new(s),
                },
            }
        }
    }

    pub fn new(s: &str) -> SSOStringRef {
        // OK to use unsafe here since s is valid UTF-8.
        unsafe { Self::new_unchecked(s.as_bytes()) }
    }
}

//------------------------------------------------------------------
// Query API

impl SSOStringRef {
    fn has_inline_data(&self) -> bool {
        unsafe { self.inner.bytes[0] != 0 }
    }

    unsafe fn as_bytes<'a>(&'a self) -> &'a [u8] {
        let s: &'a [u8];
        if self.has_inline_data() {
            let mut i = 1;
            while i < ByteSlice::SIZE_IN_BYTES && self.inner.bytes[i] != 0 {
                i += 1;
            } // Post-condition: 1 <= i <= ByteSlice::SIZE_IN_BYTES
            s = self
                .inner
                .bytes
                .get(0..i)
                .expect("Unexpected out-of-bounds byte index for SSOStringRef.");
        } else if self.inner.sref.len() == 0 {
            // If len() == 0, then the pointer may be null, which isn't allowed by as_bytes()
            s = &[];
        } else {
            s = self.inner.sref.clone().as_bytes();
        }
        return s;
    }
}

//----------------------------------------------------------------------------
// Internal API - ByteSlice type definition

/// Internal unsafe alternative to `&str` for doing bit nonsense.
///
/// Invariant: It is guaranteed that the first byte of this struct will be 0.
#[derive(Copy, Clone)]
#[repr(C)]
struct ByteSlice {
    len: usize,
    ptr: *const u8,
} // Reordering the fields here will require changing logic in SSOStringRef.

//------------------------------------------------------------------
// Constants

impl ByteSlice {
    const MAX_LEN: usize = usize::max_value() >> 8;
    const SIZE_IN_BYTES: usize = std::mem::size_of::<Self>();
}

//------------------------------------------------------------------
// Construction API

impl ByteSlice {
    #[cfg(target_endian = "little")]
    fn new(s: &[u8]) -> ByteSlice {
        std::debug_assert!(s.len() <= Self::MAX_LEN);
        ByteSlice {
            len: s.len() << 8,
            ptr: s.as_ptr(),
        }
    }

    /// The slice is assumed to point to a valid UTF-8 byte sequence,
    /// but this is not checked.
    #[cfg(target_endian = "big")]
    fn new(s: &[u8]) -> ByteSlice {
        std::debug_assert!(s.len() <= Self::MAX_LEN);
        ByteSlice {
            len: s.len(),
            ptr: s.as_ptr(),
        }
    }
}

//------------------------------------------------------------------
// Query API

impl ByteSlice {
    #[cfg(target_endian = "little")]
    fn len(&self) -> usize {
        self.len >> 8
    }

    #[cfg(target_endian = "big")]
    fn len(&self) -> usize {
        self.len
    }

    unsafe fn as_bytes<'a>(self) -> &'a [u8] {
        // https://doc.rust-lang.org/std/slice/fn.from_raw_parts.html#safety
        // says that passing a null pointer is not allowed, even for len = 0.
        debug_assert!(self.ptr != std::ptr::null());
        std::slice::from_raw_parts(self.ptr, self.len())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_istr_size() {
        assert!(std::mem::size_of::<crate::IStr>() == 4);
    }

    use std::num::NonZeroU8;

    use crate::{
        ArenaLocalIndex, ConcurrentInterner, ConcurrentInternerMemberStorage, IStr, SSOStringRef,
    };
    use quickcheck::*;
    quickcheck! {
        fn test_sso_stringref_eq1(a: String, b: String) -> bool {
            (a == b) == (SSOStringRef::new(&a) == SSOStringRef::new(&b))
        }
        fn test_sso_stringref_eq2(s: String) -> bool {
            SSOStringRef::new(&s) == SSOStringRef::new(&s.clone())
        }
    }

    quickcheck! {
        fn test_arena_local_index_roundtrip(i: u32) -> bool {
            let i = i & !(1 << 31);
            assert_eq!(Some(i), ArenaLocalIndex::new_chunk_index(i).chunk_index());
            assert_eq!(Some(i), ArenaLocalIndex::new_huge_index(i).huge_index());
            return true;
        }
    }

    quickcheck! {
        fn test_insertion(vs: Vec<String>, n: u8, near_huge_idxs: Vec<u8>, huge_idxs: Vec<u8>) -> bool {
            // Some bug in the quickcheck! macro doesn't allow mut in parameter position 🤔
            let mut vs = vs;
            if vs.len() == 0 {
                return true;
            }
            // Overwrite the vec to have some near-huge strings
            // (to exercise the chunk overflowing code paths)
            // and some huge strings (to exercise the huge string code paths)
            let chunk_size = ConcurrentInternerMemberStorage::STORAGE_CHUNK_SIZE;
            // TODO: Clean up after https://github.com/BurntSushi/quickcheck/pull/311
            // lands in quickcheck.
            let mut near_huge_idxs = near_huge_idxs;
            near_huge_idxs.truncate(3);
            for i in near_huge_idxs.iter() {
                let near_huge_idx = *i as usize;
                if near_huge_idx < vs.len() {
                    vs[near_huge_idx] = (0 .. (chunk_size - 1 - near_huge_idx)).map(|_| 'k').collect();
                }
            }
            let mut huge_idxs = huge_idxs;
            huge_idxs.truncate(3);
            for i in huge_idxs.iter() {
                let huge_idx = *i as usize;
                if huge_idx < vs.len() {
                    vs[huge_idx] = (0 .. (chunk_size + 1 + huge_idx)).map(|_| 'a').collect();
                }
            }

            let n = NonZeroU8::new((n % 8) + 1).unwrap();
            let interner = ConcurrentInterner::<ahash::RandomState>::new(n, Default::default());
            let v: Vec<Vec<IStr>> = crossbeam::scope(|scope| {
                let interner_ref = &interner;
                let vsref = &vs;
                let mut handles = vec![];
                for _ in 0 .. n.get() {
                    handles.push(scope.spawn(move |_| {
                        let mut vout = Vec::with_capacity(vsref.len());
                        let mut member = interner_ref.get_member();
                        for s in vsref.iter() {
                            vout.push(member.intern(&s));
                        }
                        vout
                    }));
                }
                handles
                .into_iter().map(|h| h.join().expect("Thread error!"))
                .collect()
            }).expect("crossbeam::scope had an error");

            let istrs0 = &v[0];
            assert!(istrs0.len() == vs.len());
            for istrs in v.get(1 ..).expect("v.len() != 0").iter() {
                assert!(istrs.len() == vs.len());
                // Check that IStr values in different threads for the same strings
                // are equal
                for (i1, i2) in istrs0.iter().zip(istrs.iter()) {
                    assert!(*i1 == *i2);
                }
            }

            // Check that get_string returns strings equivalent to the originals.
            for istrs in v.iter() {
                for (istr, s) in istrs.iter().zip(vs.iter()) {
                    assert!(interner.get_string(*istr) == *s);
                }
            }
            return true;
        }
    }
}
