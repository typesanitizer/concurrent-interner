#!/usr/bin/env python3

# Written in 2022 by Varun Gandhi <git@cutcul.us>
# To the extent possible under law, the author(s) have dedicated all copyright
# and related and neighboring rights to this software to the public domain
# worldwide. This software is distributed without any warranty.
#
# You should have received a copy of the CC0 Public Domain Dedication along with
# this software. If not, see <https://creativecommons.org/publicdomain/zero/1.0/>.

import sys
import re

time_re = re.compile("\s+time:\s*\[[0-9]+\.[0-9]+ [a-z]+ ([0-9]+\.[0-9]+ [a-z]+)")
bench_config_re = re.compile("\((.*),\s+(n = [0-9]+)")

def results():
    config_str = None
    nthreads_str = None
    output = []
    for line in sys.stdin:
        if line.startswith("intern"):
            bench_config_match = bench_config_re.search(line)
            if bench_config_match is None:
                print("Error parsing input: couldn't identify benchmark configuration in '{}'".format(line))
                continue
            config_str = bench_config_match.group(1)
            nthreads_str = bench_config_match.group(2)
            continue
        m = time_re.match(line)
        if m:
            output += [(nthreads_str, config_str, m.group(1))]
    return output

def reshape(table):
    row_keys = sorted(list(set([row[0] for row in table])))
    col_keys = sorted(list(set([row[1] for row in table])))
    d = dict([((row[0], row[1]), row[2]) for row in table ])

    result = [["" for _ in range(len(col_keys) + 1)] for _ in range(len(row_keys) + 1)]

    result[0][0] = "nthreads"
    for i, col_key in enumerate(col_keys):
        result[0][i + 1] = col_key
    for i, row_key in enumerate(row_keys):
        result[i + 1][0] = row_key
    for i, col_key in enumerate(col_keys):
        for j, row_key in enumerate(row_keys):
            result[j + 1][i + 1] = d[(row_key, col_key)]
    return result

def table_to_gfm(table):
    ncols = len(table[0])
    col_lens = []
    for i in range(ncols):
        col_lens += [max([len(row[i]) for row in table])]
    format_str = "|"
    for _ in range(ncols):
        format_str += "{}|"
    format_str += "\n"
    out = ""
    out += format_str.format(*[h.center(col_lens[i]) for (i, h) in enumerate(table[0])])
    out += format_str.format(*["".ljust(col_lens[i], "-") for (i, _) in enumerate(table[0])])
    for row in table[1:]:
        out += format_str.format(*([row[0].center(col_lens[0])] + [row[i].rjust(col_lens[i]) for i in range(1, ncols)]))
    return out.strip()

if __name__ == '__main__':
    table = results()
    table = reshape(table)
    print(table_to_gfm(table))
