# decisions-unchanged

A store holding two judgments with neither `state` nor `result`, recorded by
the koto binary built from main before those fields existed, and
`main-output.txt`, what that binary printed for these commands, in order,
one line each:

```
koto decisions show plain 000000000001
koto decisions show plain 000000000002
koto decisions list plain
koto decisions list lead --below
```

`tests/decisions_attempts_test.rs` copies `sessions/` into a fresh store, runs
the same commands with the binary under test and compares the bytes. The
machine name in the state files was replaced with `fixture-host`; no command
above prints it.
