FROM python:3.12-slim

WORKDIR /app

# Install RLM-Toolkit with all extras (fastembed default — CPU, no API keys)
RUN pip install --no-cache-dir \
    "rlm-toolkit[all]==2.3.1"

# Data directory for persistent storage
RUN mkdir -p /data

# Copy server launcher and entrypoint
COPY start_server.py /app/start_server.py
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

VOLUME ["/data"]

EXPOSE 8200

HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
  CMD python -c "import os,urllib.request; t=os.environ.get('RLM_TRANSPORT','sse'); p='/mcp' if t=='streamable-http' else '/'+t; urllib.request.urlopen(f'http://localhost:8200{p}')" || exit 1

ENTRYPOINT ["/app/entrypoint.sh"]
