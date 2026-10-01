#!/usr/bin/env bash
# Final deliverable: the rendered picture with the mastered score.
# The video stream is copied untouched; the score (assets/audio/score.wav,
# mastered by audio/master.sh) is encoded to AAC 320 kb/s at 48 kHz.
set -euo pipefail
cd "$(dirname "$0")/.."
IN="${1:-renders/render-1080p60.mp4}"
OUT="${2:-renders/zmcp-launch-1080p60.mp4}"
ffmpeg -hide_banner -loglevel error -y -i "$IN" -i assets/audio/score.wav \
  -map 0:v:0 -map 1:a:0 -c:v copy -c:a aac -b:a 320k -ar 48000 \
  -movflags +faststart -metadata title="Z-MCP — Zero Use Computer — open source" \
  -metadata comment="github.com/mhrsdev/zero-use-computer" -shortest "$OUT"
ffprobe -v error -show_entries format=duration,size:stream=codec_name,width,height,r_frame_rate,sample_rate,channels -of compact "$OUT"
ffmpeg -hide_banner -nostats -i "$OUT" -af ebur128=peak=true -f null - 2>&1 | grep -A14 Summary | grep -E "I:|LRA:|Peak:"
