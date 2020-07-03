## Fail case minimization

One of the targets is meant to minimize decoding failures. It relies on
`convert` to keep the image correct while ensuring that our own decoder fails.
This is typically a tedious process so you'll need a lot of runs.

```bash
cargo +nightly fuzz tmin fail_tmin --release --runs=65536 failing.jpeg
```
