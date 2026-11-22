Run 2026-11-20T18:27:20Z on Apple M2 Max, Darwin 25.5.0; grepum 0.5.0 (1086c38), git version 2.40.1; 50 sampled questions per project and kind.

## Tokens an agent reads per question (median)

| project | who calls X: grepum / grep | ...with their code: grepum / grep + read | read X: grepum / grep + read (complete) | read X, 60-line window (complete) | what's in a file: grepum / read |
|---|---|---|---|---|---|
| shopfront (TypeScript) | 30 / 1240 (41×) | 200 / 21414 (107×; 81 tool calls vs 1) | 64 (100%) / 1264 (100%) (20×) | 808 (100%) | 82 / 191 (2.3×) |
| datapipe (Python) | 30 / 399 (13×) | 144 / 1970 (14×; 11 tool calls vs 1) | 52 (100%) / 2259 (100%) (44×) | 2165 (100%) | 84 / 152 (1.8×) |
| arena (UE5 C++) | 37 / 868 (23×) | 179 / 6276 (35×; 41 tool calls vs 1) | 70 (100%) / 1090 (100%) (16×) | 1054 (100%) | 52 / 109 (2.1×) |
| flask (Python) | 43 / 60 (1.4×) | 86 / 13552 (159×; 4 tool calls vs 1) | 70 (100%) / 5714 (100%) (82×) | 589 (100%) | 166 / 1230 (7.4×) |
| hono (TypeScript) | 34 / 226 (6.7×) | 944 / 6885 (7.3×; 4 tool calls vs 1) | 79 (100%) / 6271 (100%) (79×) | 1209 (100%) | 64 / 419 (6.5×) |
| ripgrep (Rust) | 40 / 236 (5.9×) | 166 / 23594 (143×; 4 tool calls vs 1) | 84 (100%) / 16052 (100%) (191×) | 1606 (100%) | 234 / 2648 (11×) |
| fzf (Go) | 35 / 82 (2.3×) | 616 / 14553 (24×; 3 tool calls vs 1) | 90 (98%) / 5513 (100%) (61×) | 493 (92%) | 78 / 840 (11×) |
| leveldb (C++) | 45 / 376 (8.3×) | 396 / 32807 (83×; 10 tool calls vs 1) | 64 (100%) / 4320 (100%) (67×) | 723 (98%) | 192 / 1028 (5.4×) |

Complete: share of sampled functions shown in full (grepum shows up to 200 lines, then the start and end).

## Who calls X: correctness

| project | truth | grepum recall | grepum precision | grep recall | grep precision | precision per question: grepum / grep |
|---|---|---|---|---|---|---|
| shopfront | generator | 100.0% | 100.0% | 100.0% | 3.6% | 100.0% / 38.5% |
| datapipe | generator | 100.0% | 100.0% | 100.0% | 11.0% | 100.0% / 54.5% |
| arena | generator | 100.0% | 100.0% | 94.7% | 26.9% | 100.0% / 32.1% |
| hono | TypeScript compiler | 88.8% | 97.3% | 98.8% | 24.6% | 98.6% / 69.0% |

## Median wall time per question

| project | who calls X: grepum / grep | read X: grepum / grep | outline |
|---|---|---|---|
| shopfront | 24 ms / 9 ms | 24 ms / 9 ms | 23 ms |
| datapipe | 23 ms / 9 ms | 23 ms / 9 ms | 23 ms |
| arena | 23 ms / 8 ms | 23 ms / 8 ms | 23 ms |
| flask | 32 ms / 10 ms | 33 ms / 9 ms | 31 ms |
| hono | 27 ms / 15 ms | 28 ms / 15 ms | 27 ms |
| ripgrep | 36 ms / 10 ms | 38 ms / 9 ms | 36 ms |
| fzf | 31 ms / 8 ms | 33 ms / 8 ms | 31 ms |
| leveldb | 29 ms / 8 ms | 29 ms / 8 ms | 28 ms |
