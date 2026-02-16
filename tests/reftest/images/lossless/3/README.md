Synthetic JPEG Lossless restart-interval fixture.

- Dimensions: `64 x 32`, grayscale 16-bit.
- Encoding: JPEG Lossless (PSV=1, Pt=0), restart interval every 3 rows.
- Files:
  - `restart_psv1_dri3.jpg` (test input)
  - `restart_psv1_dri3.png` (reference output)

Generation command used:

```bash
cjpeg -lossless 1,0 -precision 16 -restart 3 -outfile restart_psv1_dri3.jpg restart_psv1_dri3.pgm
```

The source pixel values are deterministic from this formula:

`v(x, y) = ((y*257 + x*73 + (x*y)%31) * 13) % 65535`
