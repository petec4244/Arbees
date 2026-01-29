FROM python:3.11-slim

WORKDIR /app

COPY services/crypto_spot_monitor/requirements.txt ./requirements.txt
RUN pip install --no-cache-dir -r requirements.txt

COPY services/crypto_spot_monitor/ .

CMD ["python", "monitor.py"]
