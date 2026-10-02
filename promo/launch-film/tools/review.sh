#!/usr/bin/env bash
# Build review sheets from a rendered film:
#   review/contact-sheet.jpg   one frame per second, timecoded
#   review/frames/*.jpg        representative full-resolution frames
#   review/transitions/*.jpg   10 fps strips around each caused transition
#   review/audio.png           waveform + spectrogram of the mixed track
set -euo pipefail
cd "$(dirname "$0")/.."
IN="${1:-renders/zero-use-computer-launch-1080p60.mp4}"
OUT=review
mkdir -p "$OUT/frames" "$OUT/transitions"
ffmpeg -hide_banner -loglevel error -y -i "$IN" \
  -vf "fps=1,scale=384:-1,drawtext=text='%{pts\:hms}':x=6:y=6:fontsize=16:fontcolor=white:box=1:boxcolor=black@0.6,tile=6x8:padding=4:margin=4" \
  -frames:v 1 -q:v 2 "$OUT/contact-sheet.jpg"
for t in 1.4 2.9 3.9 6.3 9.9 12.6 14.9 16.95 18.9 21.6 24.9 26.5 28.9 31.4 32.6 34.9 35.9 36.7 41.0; do
  ffmpeg -hide_banner -loglevel error -y -ss "$t" -i "$IN" -frames:v 1 -q:v 2 "$OUT/frames/t$(printf %05.2f "$t").jpg"
done
for spec in "01-hit:3.3" "02-capture:5.1" "03-iris:11.9" "04-fold:17.1" "05-pause:20.4" "06-stop:24.2" "07-rewind:27.5" "08-dedupe:30.3" "09-collapse:35.4"; do
  name=${spec%%:*}; st=${spec##*:}
  ffmpeg -hide_banner -loglevel error -y -ss "$st" -t 1.2 -i "$IN" \
    -vf "fps=10,scale=480:-1,drawtext=text='%{pts\:hms}':x=6:y=6:fontsize=14:fontcolor=white:box=1:boxcolor=black@0.6,tile=4x3:padding=3" \
    -frames:v 1 -q:v 2 "$OUT/transitions/$name.jpg"
done
ffmpeg -hide_banner -loglevel error -y -i "$IN" -filter_complex \
  "[0:a]aformat=channel_layouts=mono,showwavespic=s=1800x300:colors=#8c96a1:scale=sqrt[w];[0:a]aformat=channel_layouts=mono,showspectrumpic=s=1800x400:legend=0:scale=log:fscale=log[s];[w][s]vstack" \
  "$OUT/audio.png"
echo "review sheets in $OUT/"
