### shopfront

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 88 ms | 2.22 s | 25.3x |
| re-index, nothing changed | 29 ms | 283 ms | 9.8x |
| re-index after editing 1 file | 40 ms | 2.10 s | 52.2x |
| index 4 new worktrees (total) | 207 ms | 8.92 s | 43.0x |
| who-calls query (new process) | 29 ms | 171 ms | 6.0x |
| graph of an un-checked-out commit | 16 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 490 KiB | 12860 KiB | 26.3x less |
| call-edge precision | 100.0% | 100.0% | |
| call-edge recall | 99.4% | 75.6% | |
| F1 | 0.997 | 0.861 | |
| hard case `untyped-param` found | 0/8 | 0/8 | |

### datapipe

| scenario | grepum | graphify | speedup |
|---|---:|---:|---:|
| cold index (main checkout) | 89 ms | 1.50 s | 16.8x |
| re-index, nothing changed | 25 ms | 252 ms | 10.1x |
| re-index after editing 1 file | 33 ms | 1.12 s | 34.3x |
| index 4 new worktrees (total) | 170 ms | 5.84 s | 34.4x |
| who-calls query (new process) | 24 ms | 151 ms | 6.3x |
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
| cold index (main checkout) | 71 ms | 1.62 s | 23.0x |
| re-index, nothing changed | 23 ms | 239 ms | 10.3x |
| re-index after editing 1 file | 32 ms | 1.63 s | 51.1x |
| index 4 new worktrees (total) | 163 ms | 6.35 s | 39.0x |
| who-calls query (new process) | 23 ms | 146 ms | 6.3x |
| graph of an un-checked-out commit | 12 ms | needs a checkout + full index | |
| disk used (main + 4 worktrees) | 289 KiB | 14389 KiB | 49.7x less |
| call-edge precision | 100.0% | 86.6% | |
| call-edge recall | 100.0% | 40.5% | |
| F1 | 1.000 | 0.552 | |
| hard case `callback-binding` found | 2/2 | 0/2 | |
| hard case `chained-call` found | 87/87 | 0/87 | |
| hard case `ue-rpc` found | 27/27 | 0/27 | |

