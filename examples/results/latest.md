### shopfront

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 81 ms | 2.01 s | 24.8x |
| re-index, nothing changed | 23 ms | 235 ms | 10.4x |
| re-index after editing 1 file | 33 ms | 1.88 s | 57.1x |
| index 4 new worktrees (total) | 171 ms | 7.98 s | 46.6x |
| who-calls query (new process) | 23 ms | 144 ms | 6.3x |
| graph of an un-checked-out commit | 12 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 490 KiB | 12860 KiB | 26.3x less |
| call-edge precision | 100.0% | 100.0% | |
| call-edge recall | 99.4% | 75.6% | |
| F1 | 0.997 | 0.861 | |
| hard case `untyped-param` found | 0/8 | 0/8 | |

### datapipe

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 78 ms | 1.41 s | 18.2x |
| re-index, nothing changed | 25 ms | 233 ms | 9.4x |
| re-index after editing 1 file | 35 ms | 1.05 s | 29.5x |
| index 4 new worktrees (total) | 177 ms | 5.62 s | 31.7x |
| who-calls query (new process) | 22 ms | 145 ms | 6.4x |
| graph of an un-checked-out commit | 12 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 535 KiB | 20799 KiB | 38.9x less |
| call-edge precision | 100.0% | 100.0% | |
| call-edge recall | 99.3% | 92.2% | |
| F1 | 0.996 | 0.959 | |
| hard case `chained-call` found | 60/60 | 0/60 | |
| hard case `super()` found | 60/60 | 60/60 | |
| hard case `untyped-param` found | 0/12 | 0/12 | |

### arena

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 70 ms | 1.57 s | 22.6x |
| re-index, nothing changed | 22 ms | 231 ms | 10.5x |
| re-index after editing 1 file | 31 ms | 1.56 s | 51.2x |
| index 4 new worktrees (total) | 159 ms | 6.11 s | 38.4x |
| who-calls query (new process) | 22 ms | 140 ms | 6.3x |
| graph of an un-checked-out commit | 14 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 289 KiB | 14389 KiB | 49.7x less |
| call-edge precision | 100.0% | 86.6% | |
| call-edge recall | 100.0% | 40.5% | |
| F1 | 1.000 | 0.552 | |
| hard case `callback-binding` found | 2/2 | 0/2 | |
| hard case `chained-call` found | 87/87 | 0/87 | |
| hard case `ue-rpc` found | 27/27 | 0/27 | |

