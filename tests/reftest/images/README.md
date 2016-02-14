# PNG files
All .png files in this directory were generated using `convert <input>.jpg <input>.png`

# File sources
File     | Source
-------- | ------
ycck.jpg | https://en.wikipedia.org/wiki/File:Channel_digital_image_CMYK_color.jpg
rgb.jpg  | Created from ycck.jpg using <code>convert ycck.jpg tga:- &#124; cjpeg -rgb -outfile rgb.jpg</code>
