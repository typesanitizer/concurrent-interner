// Written in 2022 by Varun Gandhi <git@cutcul.us>
// To the extent possible under law, the author(s) have dedicated all copyright
// and related and neighboring rights to this software to the public domain
// worldwide. This software is distributed without any warranty.
//
// You should have received a copy of the CC0 Public Domain Dedication along with
// this software. If not, see <https://creativecommons.org/publicdomain/zero/1.0/>.

use std::collections::HashMap;
use std::hash::BuildHasher;
use typed_arena::Arena;

//----------------------------------------------------------------------------
// Public API - Interner, type definition

pub struct Interner<'a, RS> {
    map: HashMap<&'a str, u32, RS>,
    vec: Vec<&'a str>,
    arena: &'a Arena<u8>,
}

//------------------------------------------------------------------
// Construction API

impl<RS: Default + BuildHasher> Interner<'_, RS> {
    pub fn new(arena: &Arena<u8>) -> Interner<RS> {
        Interner {
            map: HashMap::with_capacity_and_hasher(1024, Default::default()),
            vec: Vec::new(),
            arena,
        }
    }
}

//------------------------------------------------------------------
// Query API

impl<RS> Interner<'_, RS> {
    pub fn lookup(&self, idx: u32) -> &str {
        self.vec[idx as usize]
    }
}

//------------------------------------------------------------------
// Modification API

impl<RS: Default + BuildHasher> Interner<'_, RS> {
    pub fn intern(&mut self, name: &str) -> u32 {
        if let Some(&idx) = self.map.get(name) {
            return idx;
        }
        let idx = self.vec.len() as u32;
        let name = self.arena.alloc_str(name);
        self.map.insert(name, idx);
        self.vec.push(name);

        debug_assert!(self.lookup(idx) == name);
        debug_assert!(self.intern(name) == idx);

        idx
    }
}
