# LinBPQ cable experiment

Build release coppad with cpal-backend, then run `./run.sh --keep`.
`--help` lists every option. Exit codes are 0 for PASS, 1 for a failed
scenario, 2 for setup failure. Run offline checks with:

```sh
python3 -I -m unittest discover -s tools/bpq-loop -p 'test_*.py'
```

The experiment downloads the unmodified LinBPQ Linux binary from
https://www.cantab.net/users/john.wiseman/Downloads/linbpq at run time.
It prints its digest and warns when it differs from the recorded build.
The pinned Ubuntu noble i386 runtime packages are unpacked without root.
Pat v1.0.0 uses the shared hash-checked downloader in ../pat-loop/fetch-pat.sh.
Downloads stay in the cache. No binary or third-party code is vendored.
LinBPQ's working files use /dev/shm for i386-compatible inode numbers.

The test checks a local BBS delivery, not Winlink CMS forwarding.
Work was clean-room: LinBPQ ran only as an unmodified black box; no source
or HTML templates were read. See ../../docs/BPQ.md and its linked run record.
