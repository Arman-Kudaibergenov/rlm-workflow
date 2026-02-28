FROM python:3.12-slim

WORKDIR /app

# Install RLM-Toolkit with all embedding providers
# fastembed is the default — works on CPU, no API keys needed
RUN pip install --no-cache-dir \
    "rlm-toolkit[all]"

# Data directory for persistent storage
RUN mkdir -p /data

# Copy entrypoint
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

VOLUME ["/data"]

EXPOSE 8200

HEALTHCHECK --interval=30s --timeout=10s --start-period=15s --retries=3 \
  CMD python -c "import urllib.request; urllib.request.urlopen('http://localhost:8200/health')" || exit 1

ENTRYPOINT ["/app/entrypoint.sh"]
