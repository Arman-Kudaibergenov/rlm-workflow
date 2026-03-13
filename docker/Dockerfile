FROM python:3.12-slim

WORKDIR /app

# Install RLM-Toolkit with all extras (fastembed default — CPU, no API keys)
RUN pip install --no-cache-dir \
    "rlm-toolkit[all]"

# Data directory for persistent storage
RUN mkdir -p /data

# Copy server launcher and entrypoint
COPY start_server.py /app/start_server.py
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

VOLUME ["/data"]

EXPOSE 8200

HEALTHCHECK --interval=30s --timeout=10s --start-period=15s --retries=3 \
  CMD python -c "import urllib.request; urllib.request.urlopen('http://localhost:8200/sse')" || exit 1

ENTRYPOINT ["/app/entrypoint.sh"]
