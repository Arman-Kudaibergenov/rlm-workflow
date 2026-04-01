FROM python:3.12-slim

WORKDIR /app

# Install CPU-only PyTorch first (avoids 2GB+ CUDA download), then RLM-Toolkit
RUN pip install --no-cache-dir \
    torch --index-url https://download.pytorch.org/whl/cpu && \
    pip install --no-cache-dir \
    "rlm-toolkit[all]==2.3.1" watchdog

# Data directory for persistent storage
RUN mkdir -p /data

# Copy server launcher and entrypoint
COPY start_server.py /app/start_server.py
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

VOLUME ["/data"]

EXPOSE 8200

HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
  CMD python /app/healthcheck.py || exit 1

COPY healthcheck.py /app/healthcheck.py

ENTRYPOINT ["/app/entrypoint.sh"]
