# syntax=docker/dockerfile:1
# scip-python indexer. See docker/README.md for the shared design.
#
# No Python: `.python-version` names it and the entrypoint provisions it.
# Node IS bundled — scip-python is a Node application, so node is what the
# indexer needs, not something a repository pins.
#
# `git` is payload: scip-python reads the project version from it, and shells
# out to `pip list` to evaluate the project environment.

# Only the installed JS is kept; npm's install-time state stays in this stage.
FROM node:24-bookworm-slim AS sidecar
RUN npm install -g --prefix /opt/scip-python @sourcegraph/scip-python \
    && npm cache clean --force

# The node BINARY, not `node:*-slim` as a base: that image also carries npm,
# yarn and corepack, none of which run at index time.
FROM ubuntu:noble
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=entrypoint /kenn-toolchain /usr/local/bin/kenn-toolchain
COPY --from=sidecar /usr/local/bin/node /usr/local/bin/node
COPY --from=sidecar /opt/scip-python /opt/scip-python
# kenn passes the indexer as command[0], so it must resolve by name.
RUN ln -s /opt/scip-python/bin/scip-python /usr/local/bin/scip-python
WORKDIR /

ENTRYPOINT ["kenn-toolchain", "python", "--"]
