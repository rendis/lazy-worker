#!/bin/sh
# Exports one episode in every format the project uses.
# Usage: npm run export -- <tool>   (the tool is the episode's id in index.html)
set -eu
cd "$(dirname "$0")"
tool=${1:?Usage: npm run export -- <tool>}

# The episode's span comes from its mount in the root timeline.
span=$(perl -0ne 'print "$1 $2" if /id="'"$tool"'"[^>]*data-start="([\d.]+)"[^>]*data-duration="([\d.]+)"/s' index.html)
[ -n "$span" ] || { echo "No episode with id \"$tool\" in index.html" >&2; exit 1; }
start=${span% *}
end=$(echo "$span" | awk '{ print $1 + $2 }')

master=renders/$tool.mp4
npm run --silent check
npm run --silent render -- --start "$start" --end "$end" -o "$master"

# App help: a small silent loop and its last frame for reduced motion.
# The app bundles them, so they go to its tracked assets. The renders folder stays out of git.
clips=../../../src/assets/clips
mkdir -p "$clips"
ffmpeg -v error -y -i "$master" -an -vf scale=960:-2:flags=lanczos \
  -c:v libx264 -crf 26 -preset slow -pix_fmt yuv420p -movflags +faststart "$clips/$tool-loop.mp4"
ffmpeg -v error -y -sseof -0.1 -i "$master" -frames:v 1 -update 1 -vf scale=960:-2:flags=lanczos -q:v 3 "$clips/$tool-poster.jpg"

# Main README: GitHub plays a GIF from the repository.
ffmpeg -v error -y -i "$master" -loop 0 \
  -vf "fps=12,scale=480:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" \
  "../../images/$tool.gif"

ls -l "$clips/$tool-loop.mp4" "$clips/$tool-poster.jpg" "../../images/$tool.gif"
