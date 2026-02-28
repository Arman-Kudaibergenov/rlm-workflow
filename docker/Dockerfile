FROM python:3.12-slim

WORKDIR /app

# Install RLM-Toolkit and MCP server dependencies
RUN pip install --no-cache-dir \
    rlm-toolkit \
    uvicorn \
    fastapi

# Copy custom configuration and entrypoint
COPY entrypoint.sh /app/entrypoint.sh
COPY config/ /app/config/

RUN chmod +x /app/entrypoint.sh

# Persistent data volume
VOLUME ["/data"]

EXPOSE 8200

HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
  CMD curl -f http://localhost:8200/health || exit 1

ENTRYPOINT ["/app/entrypoint.sh"]
