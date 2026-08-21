#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOAD_SCRIPT="${ROOT_DIR}/scripts/load-scenarios.sh"

TARGET="${TARGET:-127.0.0.1:9889}"
HOST="${HOST:-localhost}"
H3_CLIENT_BIN="${H3_CLIENT_BIN:-${ROOT_DIR}/target/release/h3_client}"
OUT_BASE="${OUT_BASE:-${ROOT_DIR}/bench/load/matrix}"
RUN_ID="${RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
RUN_DIR="${OUT_BASE}/${RUN_ID}"

# Shared per-scenario request counts (can override per run).
BURST_REQUESTS="${BURST_REQUESTS:-3000}"
SLOW_REQUESTS="${SLOW_REQUESTS:-1000}"
LOSS_REQUESTS="${LOSS_REQUESTS:-1500}"
BURST_PATH="${BURST_PATH:-/api}"
SLOW_PATH="${SLOW_PATH:-/slow}"
LOSS_PATH="${LOSS_PATH:-/api}"
NETEM_IFACE="${NETEM_IFACE:-}"
LOSS_PERCENT="${LOSS_PERCENT:-2}"
MATRIX_PROFILES="${MATRIX_PROFILES:-}"
MATRIX_PROFILE_SELECT="${MATRIX_PROFILE_SELECT:-}"

DEFAULT_MATRIX_PROFILES=(
  "profile_1_s4_c40_20_30:0:4:40:20:30"
  "profile_2_s8_c80_40_60:0:8:80:40:60"
  "profile_3_s16_c120_60_90:0:16:120:60:90"
  "profile_4_s24_c160_80_120:0:24:160:80:120"
)

usage() {
  cat <<USAGE
Usage: scripts/load-matrix.sh

Runs four predefined load profiles and stores each profile's artifacts plus
a combined summary under:
  bench/load/matrix/<timestamp>/

Environment overrides:
  TARGET=127.0.0.1:9889
  HOST=localhost
  H3_CLIENT_BIN=./target/release/h3_client
  OUT_BASE=bench/load/matrix
  RUN_ID=custom-id

Shared request/path overrides:
  BURST_REQUESTS=3000 SLOW_REQUESTS=1000 LOSS_REQUESTS=1500
  BURST_PATH=/api SLOW_PATH=/slow LOSS_PATH=/api

Optional Linux loss injection passthrough:
  NETEM_IFACE=eth0 LOSS_PERCENT=2

Profile overrides:
  MATRIX_PROFILES="name:workers:streams:burst:slow:loss[,name:workers:streams:burst:slow:loss...]"
  MATRIX_PROFILE_SELECT="name1,name2"

Examples:
  MATRIX_PROFILES="quick:4:16:120:0:0" scripts/load-matrix.sh
  MATRIX_PROFILE_SELECT="profile_2_s8_c80_40_60" scripts/load-matrix.sh
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ ! -x "${LOAD_SCRIPT}" ]]; then
  echo "error: load script not found: ${LOAD_SCRIPT}" >&2
  exit 1
fi

if [[ ! -x "${H3_CLIENT_BIN}" ]]; then
  echo "error: h3 client binary not found: ${H3_CLIENT_BIN}" >&2
  echo "build with: cargo build --release -p impulse --bin h3_client" >&2
  exit 1
fi

mkdir -p "${RUN_DIR}"

run_profile() {
  local profile="$1"
  local workers="$2"
  local streams="$3"
  local burst_conc="$4"
  local slow_conc="$5"
  local loss_conc="$6"

  local out_dir="${RUN_DIR}/${profile}"
  mkdir -p "${out_dir}"

  echo "==> Running profile '${profile}' (workers=${workers}, streams=${streams}, burst=${burst_conc}, slow=${slow_conc}, loss=${loss_conc})"

  TARGET="${TARGET}" \
  HOST="${HOST}" \
  H3_CLIENT_BIN="${H3_CLIENT_BIN}" \
  OUT_DIR="${out_dir}" \
  LOAD_WORKERS="${workers}" \
  STREAMS_PER_WORKER="${streams}" \
  BURST_PATH="${BURST_PATH}" \
  BURST_REQUESTS="${BURST_REQUESTS}" \
  BURST_CONCURRENCY="${burst_conc}" \
  SLOW_PATH="${SLOW_PATH}" \
  SLOW_REQUESTS="${SLOW_REQUESTS}" \
  SLOW_CONCURRENCY="${slow_conc}" \
  LOSS_PATH="${LOSS_PATH}" \
  LOSS_REQUESTS="${LOSS_REQUESTS}" \
  LOSS_CONCURRENCY="${loss_conc}" \
  NETEM_IFACE="${NETEM_IFACE}" \
  LOSS_PERCENT="${LOSS_PERCENT}" \
  "${LOAD_SCRIPT}"
}

declare -a profile_rows=()
if [[ -n "${MATRIX_PROFILES}" ]]; then
  IFS=',' read -r -a profile_rows <<<"${MATRIX_PROFILES}"
else
  profile_rows=("${DEFAULT_MATRIX_PROFILES[@]}")
fi

if [[ -n "${MATRIX_PROFILE_SELECT}" ]]; then
  declare -A selected=()
  IFS=',' read -r -a selected_names <<<"${MATRIX_PROFILE_SELECT}"
  for name in "${selected_names[@]}"; do
    selected["${name}"]=1
  done

  declare -a filtered_rows=()
  for row in "${profile_rows[@]}"; do
    IFS=':' read -r profile_name _ <<<"${row}"
    if [[ -n "${selected[${profile_name}]:-}" ]]; then
      filtered_rows+=("${row}")
    fi
  done
  profile_rows=("${filtered_rows[@]}")
fi

if [[ "${#profile_rows[@]}" -eq 0 ]]; then
  echo "error: no matrix profiles selected to run" >&2
  exit 1
fi

for row in "${profile_rows[@]}"; do
  IFS=':' read -r profile_name workers streams burst_conc slow_conc loss_conc <<<"${row}"
  if [[ -z "${profile_name}" || -z "${workers}" || -z "${streams}" || -z "${burst_conc}" || -z "${slow_conc}" || -z "${loss_conc}" ]]; then
    echo "error: invalid MATRIX_PROFILES entry '${row}'" >&2
    exit 1
  fi
  run_profile "${profile_name}" "${workers}" "${streams}" "${burst_conc}" "${slow_conc}" "${loss_conc}"
done

summary_tsv="${RUN_DIR}/summary.tsv"
summary_md="${RUN_DIR}/summary.md"
summary_json="${RUN_DIR}/summary.json"
latest_note="${OUT_BASE}/latest_run_path.txt"

echo -n >"${summary_tsv}"

for profile_dir in "${RUN_DIR}"/*; do
  if [[ ! -d "${profile_dir}" ]]; then
    continue
  fi
  profile="$(basename "${profile_dir}")"
  if [[ ! -f "${profile_dir}/latest.tsv" ]]; then
    continue
  fi

  awk -F'\t' -v p="${profile}" '{printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", p,$1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12}' "${profile_dir}/latest.tsv" >>"${summary_tsv}"
done

{
  echo "# Impulse Load Matrix Summary"
  echo
  echo "- Run ID: \`${RUN_ID}\`"
  echo "- Run Dir: \`${RUN_DIR}\`"
  echo "- Target: \`${TARGET}\`"
  echo "- Host: \`${HOST}\`"
  echo "- Generated: $(date -u +'%Y-%m-%dT%H:%M:%SZ')"
  echo
  echo "| profile | scenario | path | requests | concurrency | success | errors | throughput req/s | min ms | avg ms | p50 ms | p95 ms | p99 ms |"
  echo "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
  awk -F'\t' '{
    min_ms=$9/1000000.0; avg_ms=$10/1000000.0; p50_ms=$11/1000000.0; p95_ms=$12/1000000.0; p99_ms=$13/1000000.0;
    printf("| %s | %s | %s | %s | %s | %s | %s | %s | %.3f | %.3f | %.3f | %.3f | %.3f |\n",
      $1,$2,$3,$4,$5,$6,$7,$8,min_ms,avg_ms,p50_ms,p95_ms,p99_ms)
  }' "${summary_tsv}"
} >"${summary_md}"

{
  echo '{'
  echo '  "run_id": "'"${RUN_ID}"'",'
  echo '  "run_dir": "'"${RUN_DIR}"'",'
  echo '  "target": "'"${TARGET}"'",'
  echo '  "host": "'"${HOST}"'",'
  echo '  "generated_unix_secs": '"$(date +%s)"','
  echo '  "results": ['
  awk -F'\t' 'BEGIN{first=1} {
    if (!first) printf(",\n");
    first=0;
    printf("    {\"profile\":\"%s\",\"scenario\":\"%s\",\"path\":\"%s\",\"requests\":%s,\"concurrency\":%s,\"success\":%s,\"errors\":%s,\"throughput_req_s\":%s,\"latency_ns\":{\"min\":%s,\"avg\":%s,\"p50\":%s,\"p95\":%s,\"p99\":%s}}",
      $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
  } END{printf("\n")} ' "${summary_tsv}"
  echo '  ]'
  echo '}'
} >"${summary_json}"

echo "${RUN_DIR}" >"${latest_note}"

echo "Load matrix summary: ${summary_md}"
echo "Load matrix data:    ${summary_json}"
echo "Latest run pointer:  ${latest_note}"
