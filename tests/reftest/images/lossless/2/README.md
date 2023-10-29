# JPEG Lossless data set 2

The JPEG losssless files in this data set
were collected from the DICOM-WG04 compilation of public DICOM files in
<ftp://medical.nema.org/medical/dicom/DataSets/WG04> (revision 2004/08/26),
extracting the JPEG data from the files' pixel data.

The ground truth was collected from the respective reference (uncompressed) files
and converted to 16-bit PNG files.

## Description

- **MR4**: 512x512, 12 bit precision, non-hierarchical, selection value 1
  - JPEG extracted from DICOM file `JPLL/MR4_JPLL`
  - Ground truth converted from DICOM file `REF/MR4_UNC`
- **XA1**: 1024x1024, 10 bit precision, non-hierarchical, selection value 1
  - JPEG extracted from DICOM file `JPLL/XA1_JPLL`
  - Ground truth converted from DICOM file `REF/XA1_UNC`
