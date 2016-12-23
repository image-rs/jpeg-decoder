# Change Log
All notable changes to this project will be documented in this file.
This project adheres to [Semantic Versioning](http://semver.org/).

## v0.1.10 (2016-12-23)
- Updated `rayon` to 0.6

## v0.1.9 (2016-12-12)
- Added a generic integer upsampler, which brings support for some unusual subsampling schemes, e.g. 4:1:1 (thanks @iamrohit7!)
- Made rayon optional through the `rayon` cargo feature (thanks @jackpot51!)

## v0.1.8 (2016-11-05)
* Updated rayon to version 0.5.

## v0.1.7 (2016-10-04)
- Added `UnsupportedFeature::NonIntegerSubsamplingRatio` error
- Fixed a bug which could cause certain images to fail decoding
- Fixed decoding of JPEGs which has a final RST marker in their entropy-coded data
- Avoid allocating coefficients when calling `read_info()` on progressive JPEGs

## v0.1.6 (2016-07-12)
- Added support for 16-bit quantization tables (even though the JPEG spec explicitly
  states "An 8-bit DCT-based process shall not use a 16-bit precision quantization table",
  but since libjpeg allows it there is little choice...)
- Added support for decoding files with extraneous data (this violates the JPEG spec, but libjpeg allows it)
- Fixed panic when decoding files without SOF
- Fixed bug which caused files with certain APP marker segments to fail decoding

## v0.1.5 (2016-06-22)
- Removed `euclid` and `num-rational` dependencies
- Updated `rayon` to 0.4

## v0.1.4 (2016-04-20)
- Replaced `num` with `num-rational`

## v0.1.3 (2016-04-06)
- Updated `byteorder` to 0.5

## v0.1.2 (2016-03-08)
- Fixed a bug which was causing some progressive JPEGs to fail decoding
- Performance improvements

## v0.1.1 (2016-02-29)
- Performance improvements

## v0.1.0 (2016-02-13)
- Initial release
