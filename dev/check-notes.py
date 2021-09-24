#!/usr/bin/env python3

# Written in 2022 by Varun Gandhi <git@cutcul.us>
# To the extent possible under law, the author(s) have dedicated all copyright
# and related and neighboring rights to this software to the public domain
# worldwide. This software is distributed without any warranty.
#
# You should have received a copy of the CC0 Public Domain Dedication along with
# this software. If not, see <https://creativecommons.org/publicdomain/zero/1.0/>.

import re
import subprocess
import sys

def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)

def default_main():
    note_re_str = r'\[(REF )?NOTE: ([a-zA-Z0-9\-]+)\]'
    note_re = re.compile(note_re_str)
    git_grep = subprocess.run(['git', 'grep', '-E', note_re_str], capture_output=True, encoding='utf-8')
    notes = {}
    for match in re.finditer(note_re, git_grep.stdout):
        ref = match.group(1)
        note = match.group(2)
        if ref:
            if note in notes:
                notes[note].add('ref')
            else:
                notes[note] = {'ref'}
        else:
            if note in notes:
                notes[note].add('def')
            else:
                notes[note] = {'def'}

    hadError = False
    for note, defref in notes.items():
        if 'ref' in defref:
            if 'def' not in defref:
                hadError = True
                eprint('Missing definition for reference {}'.format(note))
        else:
            assert('def' in defref)
            eprint('No references for definition {}; maybe delete the definition?'.format(note))

    if hadError:
        sys.exit(1)

if __name__ == '__main__':
    default_main()
