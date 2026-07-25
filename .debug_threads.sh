#!/bin/sh
# Debug: stato dei thread del processo 769 (bench congelato) nel VM Docker.
for t in /proc/769/task/*; do
  tid=${t##*/}
  state=$(grep '^State:' "$t/status" | awk '{print $2}')
  wchan=$(cat "$t/wchan" 2>/dev/null)
  name=$(cat "$t/comm" 2>/dev/null)
  echo "tid=$tid state=$state wchan=$wchan comm=$name"
done
echo "--- children of 769 ---"
for p in /proc/[0-9]*; do
  ppid=$(awk '{print $4}' "$p/stat" 2>/dev/null)
  if [ "$ppid" = "769" ]; then echo "child ${p#/proc/} $(cat $p/comm)"; fi
done
