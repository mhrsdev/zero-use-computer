#!/usr/bin/env bash
# Build review sheets from a rendered film:
#   review/contact-sheet.jpg   one frame per second, timecoded
#   review/frames/*.jpg        representative full-resolution frames
#   review/transitions/*.jpg   10 fps strips around each caused transition
#   review/audio.png           waveform + spectrogram of the mixed track
set -euo pipefail
cd "$(dirname "$0")/.."
IN="${1:-renders/zero-use-computer-film2-1080p60.mp4}"
OUT=review
mkdir -p "$OUT/frames" "$OUT/transitions"
ffmpeg -hide_banner -loglevel error -y -i "$IN" \
  -vf "fps=2,scale=384:-1,drawtext=text='%{pts\:hms}':x=6:y=6:fontsize=16:fontcolor=white:box=1:boxcolor=black@0.6,tile=8x8:padding=4:margin=4" \
  -frames:v 1 -q:v 2 "$OUT/contact-sheet.jpg"
for t in 0.59 0.98 1.97 3.25 4.14 5.12 7.78 8.47 9.94 10.44 11.62 12.11 13.10 13.59 14.08 14.97 16.74 17.23 18.31 19.10 21.27 22.55 23.83 24.42 26.09 29.54; do
  ffmpeg -hide_banner -loglevel error -y -ss "$t" -i "$IN" -frames:v 1 -q:v 2 "$OUT/frames/t$(printf %05.2f "$t").jpg"
done
for spec in "01-intro:0.30" "02-drop:3.35" "03-capture:7.19" "04-ticker:8.86" "05-click:11.32" "06-path:12.11" "07-rewind:16.15" "08-pause:18.41" "09-stop:20.68" "10-reveal:25.40"; do
  name=${spec%%:*}; st=${spec##*:}
  ffmpeg -hide_banner -loglevel error -y -ss "$st" -t 1.2 -i "$IN" \
    -vf "fps=10,scale=480:-1,drawtext=text='%{pts\:hms}':x=6:y=6:fontsize=14:fontcolor=white:box=1:boxcolor=black@0.6,tile=4x3:padding=3" \
    -frames:v 1 -q:v 2 "$OUT/transitions/$name.jpg"
done
ffmpeg -hide_banner -loglevel error -y -i "$IN" -filter_complex \
  "[0:a]aformat=channel_layouts=mono,showwavespic=s=1800x300:colors=#8c96a1:scale=sqrt[w];[0:a]aformat=channel_layouts=mono,showspectrumpic=s=1800x400:legend=0:scale=log:fscale=log[s];[w][s]vstack" \
  "$OUT/audio.png"
echo "review sheets in $OUT/"
