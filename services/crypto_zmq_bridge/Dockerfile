FROM python:3.11-slim

WORKDIR /app

RUN pip install --no-cache-dir \
    pyzmq==26.0.1 \
    loguru

COPY bridge.py .

CMD ["python", "bridge.py"]
