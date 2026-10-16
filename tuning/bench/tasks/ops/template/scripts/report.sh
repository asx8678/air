#!/bin/sh
# Count 5xx responses per endpoint across all access logs.
set -e
mkdir -p reports
cat logs/access.log* \
  | awk '$9 ~ /^[45]/ {print $7}' \
  | sort | uniq -c | sort -rn \
  | awk 'BEGIN {print "endpoint,count"} {print $2 "," $1}' > reports/errors.csv
