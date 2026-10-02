# ABOUTME: Default sandbox image for the docker provider: git, tmux, Node.js, and Claude Code.
# ABOUTME: Build it with: docker build -t beam-base images/base
FROM node:22-bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends git gh tmux ca-certificates curl build-essential python3 python3-venv \
 && rm -rf /var/lib/apt/lists/* \
 && npm install -g @anthropic-ai/claude-code pnpm
