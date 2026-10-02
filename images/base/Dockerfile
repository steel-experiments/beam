# ABOUTME: Default sandbox image for the docker provider: git, tmux, Node.js, pnpm, and Claude Code.
# ABOUTME: Build it with: beam --build-image (or docker build -t beam-base images/base)
ARG NODE_MAJOR=22
FROM node:${NODE_MAJOR}-bookworm-slim
ARG NODE_MAJOR
ARG PNPM_VERSION=11.11.0
# Claude Code follows its latest release. The build time label shows the age of the image.
ARG CLAUDE_CODE_VERSION=latest
ARG BEAM_BUILT=0
# Corepack runs the pnpm version from a project's packageManager field without a prompt.
# Its cache does not depend on HOME, because Beam sets HOME for each transfer.
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0 \
    COREPACK_HOME=/usr/local/share/corepack
RUN apt-get update \
 && apt-get install -y --no-install-recommends git gh tmux ca-certificates curl build-essential python3 python3-venv \
 && rm -rf /var/lib/apt/lists/* \
 && npm install -g "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
 && corepack enable pnpm \
 && corepack install -g "pnpm@${PNPM_VERSION}" \
 && chmod -R a+rX "$COREPACK_HOME"
LABEL beam.built="${BEAM_BUILT}" \
      beam.node="${NODE_MAJOR}" \
      beam.pnpm="${PNPM_VERSION}" \
      beam.claude-code="${CLAUDE_CODE_VERSION}"
