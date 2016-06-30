# PNG files
All .png files in this directory were generated using `convert <input>.jpg <input>.png`

# File sources
File              | Source
------------------| ------
16bit-qtables.jpg | Created using <code>convert mozilla/jpg-size-1x1.png tga:- &#124; cjpeg -quality 10 -outfile 16bit-qtables.jpg</code>
restarts.jpg      | `mozilla/jpg-size-33x33.jpg` with added restart markers.
rgb.jpg           | Created from `ycck.jpg` using <code>convert ycck.jpg tga:- &#124; cjpeg -rgb -outfile rgb.jpg</code>
ycck.jpg          | https://en.wikipedia.org/wiki/File:Channel_digital_image_CMYK_color.jpg
