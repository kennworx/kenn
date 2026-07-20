# syntax=docker/dockerfile:1
# kenn-dotnet (C#) indexer. See docker/README.md for the shared design.
#
# No .NET SDK: `global.json` names it and the entrypoint provisions it.

# Self-contained, so the sidecar carries its own net10 runtime and runs against
# whatever SDK major the workspace pinned — including an older one.
FROM mcr.microsoft.com/dotnet/sdk:10.0 AS sidecar
ARG TARGETARCH
COPY indexers/kenn-dotnet /src
RUN case "$TARGETARCH" in \
      amd64) rid=linux-x64 ;; \
      arm64) rid=linux-arm64 ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && dotnet publish /src/kenn-dotnet.csproj -c Release -r "$rid" --self-contained true \
      -o /opt/kenn-dotnet --nologo

# ICU is required: globalization is on, and without it Roslyn fails on
# culture-sensitive comparisons rather than degrading. `git` is payload — the
# indexer reads the target's VCS metadata.
FROM ubuntu:noble
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
         git ca-certificates libicu74 libstdc++6 libgcc-s1 zlib1g tzdata \
    && rm -rf /var/lib/apt/lists/*
COPY --from=entrypoint /kenn-toolchain /usr/local/bin/kenn-toolchain
COPY --from=sidecar /opt/kenn-dotnet /opt/kenn-dotnet
# kenn passes the indexer as command[0], so it must resolve by name.
RUN ln -s /opt/kenn-dotnet/kenn-dotnet /usr/local/bin/kenn-dotnet
ENV DOTNET_NOLOGO=true \
    DOTNET_RUNNING_IN_CONTAINER=true \
    NUGET_XMLDOC_MODE=skip
WORKDIR /

ENTRYPOINT ["kenn-toolchain", "csharp", "--"]
