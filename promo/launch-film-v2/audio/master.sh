#!/usr/bin/env bash
# Edit the music and place the sound effects (score.py), then master: static gain into an
# oversampled peak limiter, landing near -16 LUFS integrated, under -1 dBTP after AAC. The
# track is already mastered, so it gets only a couple of dB of peak limiting.
# No dynamic loudness processing, so the film's dynamics stay as composed.
set -euo pipefail
cd "$(dirname "$0")"
python3 score.py
I=$(ffmpeg -hide_banner -nostats -i mix-raw.wav -af ebur128 -f null - 2>&1 | grep -A3 "Integrated loudness" | grep "I:" | awk '{print $2}')
GAIN=$(python3 -c "print(round(-16.0 - ($I), 2))")
echo "raw integrated $I LUFS -> gain $GAIN dB"
mkdir -p ../assets/audio
ffmpeg -hide_banner -loglevel error -y -i mix-raw.wav \
  -af "volume=${GAIN}dB,aresample=192000,alimiter=limit=0.79:attack=2:release=80:level=false,aresample=48000" \
  -c:a pcm_s16le ../assets/audio/score.wav
ffmpeg -hide_banner -nostats -i ../assets/audio/score.wav -af ebur128=peak=true -f null - 2>&1 | grep -A14 Summary | grep -E "I:|LRA:|Peak:"
