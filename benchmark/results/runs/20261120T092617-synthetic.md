Run 2026-11-20T09:26:17Z on Apple M2 Max, Darwin 25.5.0; grepum 0.1.0 (01bc638), graphify 0.9.112; median of 3 runs.

### shopfront (synthetic)

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 78 ms | 2.06 s | 26.4x |
| re-index, nothing changed | 23 ms | 243 ms | 10.5x |
| re-index after editing 1 file | 33 ms | 1.97 s | 58.8x |
| index 4 new worktrees (total) | 173 ms | 8.28 s | 47.9x |
| who-calls query (new process) | 23 ms | 155 ms | 6.6x |
| graph of an un-checked-out commit | 13 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 490 KiB | 12860 KiB | 26.3x less |
| call-edge precision | 100.0% | 100.0% | |
| call-edge recall | 99.4% | 75.6% | |
| F1 | 0.997 | 0.861 | |
| hard case `untyped-param` found | 0/8 | 0/8 | |

### datapipe (synthetic)

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 76 ms | 1.43 s | 18.8x |
| re-index, nothing changed | 23 ms | 245 ms | 10.8x |
| re-index after editing 1 file | 33 ms | 1.10 s | 33.7x |
| index 4 new worktrees (total) | 166 ms | 5.81 s | 35.0x |
| who-calls query (new process) | 23 ms | 152 ms | 6.6x |
| graph of an un-checked-out commit | 13 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 535 KiB | 20799 KiB | 38.9x less |
| call-edge precision | 100.0% | 100.0% | |
| call-edge recall | 99.3% | 92.2% | |
| F1 | 0.996 | 0.959 | |
| hard case `chained-call` found | 60/60 | 0/60 | |
| hard case `super()` found | 60/60 | 60/60 | |
| hard case `untyped-param` found | 0/12 | 0/12 | |

### arena (synthetic)

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 68 ms | 1.60 s | 23.4x |
| re-index, nothing changed | 23 ms | 237 ms | 10.3x |
| re-index after editing 1 file | 31 ms | 1.62 s | 52.0x |
| index 4 new worktrees (total) | 158 ms | 6.39 s | 40.4x |
| who-calls query (new process) | 23 ms | 155 ms | 6.8x |
| graph of an un-checked-out commit | 12 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 289 KiB | 14389 KiB | 49.7x less |
| call-edge precision | 100.0% | 86.6% | |
| call-edge recall | 100.0% | 40.5% | |
| F1 | 1.000 | 0.552 | |
| hard case `callback-binding` found | 2/2 | 0/2 | |
| hard case `chained-call` found | 87/87 | 0/87 | |
| hard case `ue-rpc` found | 27/27 | 0/27 | |

