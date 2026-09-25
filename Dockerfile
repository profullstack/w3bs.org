FROM node:24-bookworm-slim
WORKDIR /app
COPY --chown=node:node package.json package-lock.json ./
RUN npm ci --omit=dev --no-audit --no-fund && npm cache clean --force
COPY --chown=node:node src ./src
COPY --chown=node:node native ./native
COPY --chown=node:node public ./public
COPY --chown=node:node fixtures ./fixtures
COPY --chown=node:node docs ./docs
RUN mkdir -p /data && chown node:node /data
USER node
ENV NODE_ENV=production HOST=0.0.0.0 PORT=3000 W3BS_DATA_DIR=/data
EXPOSE 3000
CMD ["node", "src/bootstrap.mjs"]
