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
