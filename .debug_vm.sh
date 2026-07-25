#!/bin/sh
echo "== meminfo =="
grep -E "MemTotal|MemFree|MemAvailable|SwapTotal|SwapFree|Committed_AS|HugePages" /proc/meminfo
echo "== khugepaged =="
cat /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null
cat /sys/kernel/mm/transparent_hugepage/defrag 2>/dev/null
echo "== thread 769 stack =="
cat /proc/769/task/769/stack 2>/dev/null || echo "stack non leggibile"
echo "== wchan resample =="
for t in /proc/769/task/*; do
  echo "tid=${t##*/} wchan=$(cat $t/wchan 2>/dev/null) $(grep '^State:' $t/status | awk '{print $2}')"
done
