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
for t in 0.6 1.0 2.0 3.3 4.2 5.2 7.9 8.6 10.1 10.6 11.8 12.3 13.3 13.8 14.3 15.2 17.0 17.5 18.6 19.4 21.6 22.9 24.2 24.8 26.5 30.0; do
  ffmpeg -hide_banner -loglevel error -y -ss "$t" -i "$IN" -frames:v 1 -q:v 2 "$OUT/frames/t$(printf %05.2f "$t").jpg"
done
for spec in "01-intro:0.3" "02-drop:3.4" "03-capture:7.3" "04-ticker:9.0" "05-click:11.5" "06-path:12.3" "07-rewind:16.4" "08-pause:18.7" "09-stop:21.0" "10-reveal:25.8"; do
  name=${spec%%:*}; st=${spec##*:}
  ffmpeg -hide_banner -loglevel error -y -ss "$st" -t 1.2 -i "$IN" \
    -vf "fps=10,scale=480:-1,drawtext=text='%{pts\:hms}':x=6:y=6:fontsize=14:fontcolor=white:box=1:boxcolor=black@0.6,tile=4x3:padding=3" \
    -frames:v 1 -q:v 2 "$OUT/transitions/$name.jpg"
done
ffmpeg -hide_banner -loglevel error -y -i "$IN" -filter_complex \
  "[0:a]aformat=channel_layouts=mono,showwavespic=s=1800x300:colors=#8c96a1:scale=sqrt[w];[0:a]aformat=channel_layouts=mono,showspectrumpic=s=1800x400:legend=0:scale=log:fscale=log[s];[w][s]vstack" \
  "$OUT/audio.png"
echo "review sheets in $OUT/"
