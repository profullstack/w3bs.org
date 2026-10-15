# syntax=docker/dockerfile:1
# w3bs.org reference stack on Bun. dev2's compose (docker-compose.app.yml,
# `dockerfile: Dockerfile`) builds this file from the repo root on every merge.
# Plain Express server with no build step: install production dependencies from
# bun.lock, then run src/bootstrap.mjs directly.
FROM oven/bun:1.4.2-slim AS deps
WORKDIR /app
# bunfig.toml turns lifecycle scripts off: ThreatCrush's daemon-only better-sqlite3
# would otherwise try to compile, and the scanner does not need it.
COPY package.json bun.lock bunfig.toml ./
RUN bun install --frozen-lockfile --production

FROM oven/bun:1.4.2-slim AS runtime
WORKDIR /app
ENV NODE_ENV=production HOST=0.0.0.0 PORT=3000 W3BS_DATA_DIR=/data
COPY --from=deps --chown=bun:bun /app ./
COPY --chown=bun:bun src ./src
COPY --chown=bun:bun native ./native
COPY --chown=bun:bun public ./public
COPY --chown=bun:bun fixtures ./fixtures
COPY --chown=bun:bun docs ./docs
COPY --chown=bun:bun catalog ./catalog
COPY --chown=bun:bun registry ./registry
# Agentic Desktop Layer installer served at /desktop/install.sh.
COPY --chown=bun:bun desktop/packaging ./desktop/packaging
# Operators re-scan a package by hand with: bun scripts/scan-packages.mjs --record <id>
COPY --chown=bun:bun scripts/scan-packages.mjs ./scripts/scan-packages.mjs
# Production storage is PostgreSQL (DATABASE_URL). Without it the store falls
# back to SQLite in W3BS_DATA_DIR, so keep that directory writable by uid 1000.
RUN mkdir -p /data && chown bun:bun /data
USER bun
EXPOSE 3000
# The app answers only its own hosts (ALLOWED_HOSTS / PUBLIC_ORIGIN), so ask
# with the public origin's Host header.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD bun -e "const h=new URL(process.env.PUBLIC_ORIGIN||'http://localhost').hostname;fetch('http://127.0.0.1:'+(process.env.PORT||3000)+'/healthz',{headers:{host:h}}).then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"
CMD ["bun", "src/bootstrap.mjs"]
