# Fuzzing harnesses

## Using the fuzzer

Install afl:

    $ cargo install afl

Build fuzz target:

    $ cargo afl build --release --bin fuzz_<format>

Run afl:

    $ mkdir out/
    $ cargo afl fuzz -i in/ -o out/ target/release/fuzz_<target>

To reproduce a crash:

    $ cargo run --bin reproduce_<target>

Note: You should also try fuzzing in debug mode, since things like overflow
checks don't happen in release mode. (Release mode is much faster though.)
