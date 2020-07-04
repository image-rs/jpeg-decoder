## Regression test case minimization

One of the targets is meant to minimize decoding failure regression tests. It
relies on comparing the output of the local decoder against the version on
Github's default branch. One of the goals is to remove copyrighted content from
a reproduction case as well.

```bash
cargo +nightly fuzz tmin regression known_reported_reproduction.jpeg
```

## Reftest generation

The second fuzzing target will help you generate an image for reftest. It
relies on `mozjpeg` to keep the image correct while ensuring that a) the
previous decoder failed and b) the current decoder succeeds and agrees with the
output of `mozjpeg`. This is typically a tedious process so you'll need a lot
of runs but the lib is really fast as everything happens in process.
Unfortunately, it is also quite finicky as the color treatment is not the same
with some deviation and seemingly `mozjpeg` is quite strict in what it accepts.
You may need to manually edit the file structure and rely on this only for
filling in some colors and index bytes that work.

To work, create a new folder, copy a known minimized regression test case in
and let it go to work. You might want to supply some other jpeg samples for
convenience.

```bash
tmpdir="$(mktemp)"
cp -t "$tmpdir" minimized_reproduction.jpg
cargo +nightly fuzz run fail_tmin "$tmpdir"
```
