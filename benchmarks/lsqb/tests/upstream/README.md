# Upstream validator fixtures

`expected-output.csv` is the byte-for-byte LSQB oracle from commit
`242cb2fd31340ca688954cb94794d74c0d5b6f92`; its SHA-256 is
`f2467b14cd6a060e8513d5357471ae6cff486c2f5e38074febe08a4cf4db0d3a`.
It remains subject to LSQB's Apache-2.0
[`LICENSE.txt`](LICENSE.txt) and [`NOTICE.txt`](NOTICE.txt), copied byte-for-byte
from that pinned source archive.

The `upstream-ladybug-*.csv` files are synthetic Grust test observations. They
exercise the same six-column format and published counts without representing
benchmark measurements.
