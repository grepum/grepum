Run 2026-11-20T09:26:17Z on Apple M2 Max, Darwin 25.5.0; grepum 0.1.0 (01bc638), graphify 0.9.112; median of 3 runs.

### flask (Python, [pallets/flask](https://github.com/pallets/flask) @ 3.1.3)

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 64 ms | 2.02 s | 31.6x |
| re-index, nothing changed | 24 ms | 951 ms | 40.4x |
| re-index after editing 1 file | 36 ms | 1.57 s | 44.0x |
| index 4 new worktrees (total) | 195 ms | 8.05 s | 41.3x |
| who-calls query (new process) | 25 ms | 189 ms | 7.7x |
| graph of an un-checked-out commit | 12 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 875 KiB | 30584 KiB | 34.9x less |
| call edges reported (no ground truth) | 1237 | 299 | |

### hono (TypeScript, [honojs/hono](https://github.com/honojs/hono) @ v4.13.19)

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 170 ms | 4.59 s | 27.0x |
| re-index, nothing changed | 26 ms | 353 ms | 13.6x |
| re-index after editing 1 file | 49 ms | 2.40 s | 48.9x |
| index 4 new worktrees (total) | 321 ms | 18.47 s | 57.5x |
| who-calls query (new process) | 27 ms | 227 ms | 8.4x |
| graph of an un-checked-out commit | 13 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 2128 KiB | 38300 KiB | 18.0x less |
| call edges reported (no ground truth) | 1504 | 850 | |

### ripgrep (Rust, [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) @ 15.2.0)

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 93 ms | 2.34 s | 25.2x |
| re-index, nothing changed | 25 ms | 269 ms | 11.0x |
| re-index after editing 1 file | 38 ms | 2.47 s | 65.5x |
| index 4 new worktrees (total) | 223 ms | 9.38 s | 42.1x |
| who-calls query (new process) | 25 ms | 246 ms | 9.8x |
| graph of an un-checked-out commit | 13 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 1753 KiB | 62920 KiB | 35.9x less |
| call edges reported (no ground truth) | 2406 | 2673 | |

### fzf (Go, [junegunn/fzf](https://github.com/junegunn/fzf) @ v0.75.0)

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 95 ms | 1.74 s | 18.3x |
| re-index, nothing changed | 24 ms | 256 ms | 10.6x |
| re-index after editing 1 file | 39 ms | 1.49 s | 38.0x |
| index 4 new worktrees (total) | 203 ms | 6.89 s | 34.0x |
| who-calls query (new process) | 24 ms | 185 ms | 7.7x |
| graph of an un-checked-out commit | 13 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 831 KiB | 52680 KiB | 63.4x less |
| call edges reported (no ground truth) | 1852 | 2304 | |

### leveldb (C++, [google/leveldb](https://github.com/google/leveldb) @ 1.23)

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 97 ms | 2.56 s | 26.4x |
| re-index, nothing changed | 27 ms | 253 ms | 9.4x |
| re-index after editing 1 file | 56 ms | 2.08 s | 37.4x |
| index 4 new worktrees (total) | 261 ms | 10.23 s | 39.2x |
| who-calls query (new process) | 24 ms | 208 ms | 8.6x |
| graph of an un-checked-out commit | 14 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 1370 KiB | 60311 KiB | 44.0x less |
| call edges reported (no ground truth) | 2840 | 2091 | |

