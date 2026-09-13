# Hermes: the test cluster's utility API. Standard library only, so the image
# is the base python with one file copied in — no build step, no lockfile.
FROM python:3.12-slim
WORKDIR /app
COPY server.py /app/server.py
EXPOSE 8080
CMD ["python", "-u", "/app/server.py"]
