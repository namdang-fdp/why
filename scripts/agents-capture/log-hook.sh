#!/usr/bin/env bash
# Capture hook: appends one JSON line per invocation to $WHY_CAPTURE_LOG.
# Prints nothing and exits 0 unless a behaviour-experiment knob is set:
#   WHY_CAPTURE_STDOUT=<text>  print text to stdout (does the harness inject it?)
#   WHY_CAPTURE_STDERR=<text>  print text to stderr (the "blocking reason" channel)
#   WHY_CAPTURE_ONLY=<Event,..> apply the knobs only to those events
#   WHY_CAPTURE_EXIT=<n>       exit with n
#   WHY_CAPTURE_SLEEP=<secs>   sleep before exiting (timeout experiments)
# Usage in a hook config: log-hook.sh <harness-label>
exec python3 -I "$(dirname "$0")/log-hook.py" "$@"
