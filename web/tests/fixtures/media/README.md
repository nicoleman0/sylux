# Synthetic media

Generate the eight-second clips from FFmpeg test sources. The browser smoke suite reads the checked-in outputs and needs no FFmpeg installation.

```sh
ffmpeg -f lavfi -i 'testsrc2=size=96x64:rate=8:duration=8' -an -c:v libx264 -profile:v baseline -level:v 3.0 -pix_fmt yuv420p -g 8 -sc_threshold 0 -threads 1 -movflags +faststart pattern.mp4
ffmpeg -f lavfi -i 'sine=frequency=440:sample_rate=22050:duration=8' -c:a libmp3lame -b:a 32k -map_metadata -1 tone.mp3
```
