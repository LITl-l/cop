#!/usr/bin/env bash
# hostile — a deliberately non-conformant plugin, used to verify that the HOST is
# robust. Every mode corresponds to a way a real plugin misbehaves in production.
# It is a shell script on purpose: the COP contract is small enough that a
# hostile implementation needs no toolchain at all.
#
# covers: COP-ERR-DRAIN COP-TIME-HOST COP-TIME-EXPIRE COP-WIRE-MAXLINE COP-WIRE-COMPACT
set -u
MODE="${1:-${COP_HOSTILE_MODE:-normal}}"
# covers: COP-FD-USE — write to fd 3 when the host opened it, else stdout.
OUT=1
if [ -n "${COP_PROTOCOL_FD:-}" ] && [ "${COP_PROTOCOL_FD}" -gt 2 ] 2>/dev/null; then OUT="${COP_PROTOCOL_FD}"; fi
emit() { printf '%s\n' "$1" >&"$OUT"; }
ENGINE='"engine":{"name":"hostile","version":"0.1.0"}'
caps() { emit "$(printf '{"cop":"0.1","id":"%s","status":"ok","confidence":"structural","evidence":[],"result":{"cop_versions":["0.1"],"engine":{"name":"hostile","version":"0.1.0"},"languages":["*"],"verbs":{"exists":{"confidence":"structural","requires_build":false}}},%s}' "$1" "$ENGINE")"; }

while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  verb=$(printf '%s' "$line" | sed -n 's/.*"verb"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  case "$MODE" in
    exit-immediately)   exit 1 ;;
    # SPEC 13.2: not misbehaviour — a plugin declining the suite. The harness
    # must record UNSUPPORTED and stay green.
    exit-127)           exit 127 ;;
    exit-mid-message)   printf '{"cop":"0.1","id":"%s","status":"ok","confi' "$id"; exit 0 ;;
    hang)               sleep 3600 ;;
    wrong-id)           printf '{"cop":"0.1","id":"ZZZ-not-yours","status":"unsupported","reason":"x",%s}\n' "$ENGINE" ;;
    duplicate-response) printf '{"cop":"0.1","id":"%s","status":"unsupported","reason":"a",%s}\n{"cop":"0.1","id":"%s","status":"unsupported","reason":"b",%s}\n' "$id" "$ENGINE" "$id" "$ENGINE" ;;
    invalid-utf8)       printf '{"cop":"0.1","id":"%s","status":"unsupported","reason":"\xff\xfe",%s}\n' "$id" "$ENGINE" ;;
    pretty-printed)     printf '{\n  "cop": "0.1",\n  "id": "%s",\n  "status": "unsupported",\n  "reason": "multi-line",\n  %s\n}\n' "$id" "$ENGINE" ;;
    huge-single-line)   printf '{"cop":"0.1","id":"%s","status":"unsupported","reason":"' "$id"; head -c 20000000 /dev/zero | tr '\0' 'x'; printf '",%s}\n' "$ENGINE" ;;
    not-json)           printf 'this is not json at all\n' ;;
    stderr-flood)
        # The classic deadlock: fill the stderr pipe (64 KiB on Linux) while the
        # host may still be waiting to read the protocol channel.
        head -c 4000000 /dev/zero | tr '\0' 'E' >&2
        if [ "$verb" = "capabilities" ]; then caps "$id"; else
          printf '{"cop":"0.1","id":"%s","status":"unsupported","reason":"flooded",%s}\n' "$id" "$ENGINE"; fi ;;
    stdout-pollution)
        echo "WARNING: some library printed this to stdout"
        if [ "$verb" = "capabilities" ]; then caps "$id"; else
          printf '{"cop":"0.1","id":"%s","status":"unsupported","reason":"noisy",%s}\n' "$id" "$ENGINE"; fi ;;
    *)  # covers: COP-BATCH-ACCEPT — a batch line carries several ids.
        ids=$(printf '%s' "$line" | grep -oE '"id"[[:space:]]*:[[:space:]]*"[^"]*"' | sed 's/.*"\([^"]*\)"$/\1/')
        for one in $ids; do
          if [ "$verb" = "capabilities" ]; then caps "$one"; else
            emit "$(printf '{"cop":"0.1","id":"%s","status":"unsupported","reason":"normal",%s}' "$one" "$ENGINE")"; fi
        done ;;
  esac
done
exit 0
