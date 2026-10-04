#!/bin/bash
set -a
source /home/rahim/projects/ndvi-service/.env
set +a
export RUST_LOG=debug
export PORT=8082
/home/rahim/projects/ndvi-service/target/release/ndvi-service
