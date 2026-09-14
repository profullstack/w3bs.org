FROM node:24-bookworm-slim
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev --no-audit --no-fund && npm cache clean --force
COPY src ./src
COPY native ./native
COPY public ./public
COPY fixtures ./fixtures
COPY docs ./docs
RUN mkdir -p /data && chown node:node /data
USER node
ENV NODE_ENV=production HOST=0.0.0.0 PORT=3000 W3BS_DATA_DIR=/data
EXPOSE 3000
CMD ["node", "src/bootstrap.mjs"]
