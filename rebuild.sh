#!/bin/bash
# Rebuild rlm-rs and restart the test instance. Usage: bash rebuild.sh
set -e
cd /opt/rlm-rs
. /root/.cargo/env
CARGO_BUILD_JOBS=1 cargo build --release 2>&1 | tail -3
systemctl restart rlm-rs-test.service
sleep 4
systemctl is-active rlm-rs-test.service