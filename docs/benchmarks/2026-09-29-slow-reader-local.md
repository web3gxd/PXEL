# Local paused WebSocket subscriber stress, 2026-09-29

`just stress-slow-reader` ran twice against a release server and real local
PostgreSQL 18.6 on Windows 10.0.26100 / Intel Core Ultra 9 185H. A writer and
a subscriber connected to one newly created document. After receiving the
subscription barrier, the test paused the subscriber's underlying TCP socket.
The writer then sent 512 sequential 30,005-byte text edits to one cell,
spaced by 22 ms to stay under the development server's global 64/s limit.
Each command's ACK was checked for the next version. The final authoritative
projection had version 513 and the last expected value.

| Run | Test duration | Writer commits | Final head | Paused socket after resume |
| --- | ------------: | -------------: | ---------: | -------------------------- |
| 1   |       19.57 s |        512/512 |        513 | Closed                     |
| 2   |       19.17 s |        512/512 |        513 | Closed                     |

This shows that a non-reading subscriber did not prevent the document writer
from committing approximately 15.36 MB of events on this topology. The slow
socket was disconnected before the test resumed it. The current logs do not
identify whether the connection ended because the outbound queue filled or a
socket send timed out. Unit tests separately check both outbound queue limits
(160 frames and 2 MiB). This stress test does not measure cross-host behavior,
precise queue depth, actor latency distribution, or multiple slow subscribers.
