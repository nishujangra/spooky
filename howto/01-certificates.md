# How to Set Up TLS Certificates

Spooky requires TLS certificates for both the QUIC/HTTP3 listener and the HTTP/1.1+HTTP/2 bootstrap TLS listener.

- **Certificate format:** PEM X.509 (`-----BEGIN CERTIFICATE-----`)
- **Key format:** PEM private key — both PKCS#8 (`-----BEGIN PRIVATE KEY-----`) and PKCS#1 (`-----BEGIN RSA PRIVATE KEY-----`) are accepted
- Both are validated at startup — spooky exits if either is missing or malformed

---

## Option 1: Let's Encrypt with Certbot (Standalone)

Use when no other service is running on port 80, or when you want a fresh independent cert.

```bash
# Stop whatever is on port 80 first (e.g. sudo systemctl stop caddy/nginx/apache)

sudo apt install -y certbot

sudo certbot certonly --standalone \
  -d example.com \
  --email admin@example.com \
  --agree-tos \
  --non-interactive
```

Let's Encrypt issues PKCS#1 keys — convert to PKCS#8 which Spooky requires:

```bash
sudo mkdir -p /etc/spooky/certs

sudo openssl pkcs8 -topk8 -nocrypt \
  -in /etc/letsencrypt/live/example.com/privkey.pem \
  -out /etc/spooky/certs/privkey.pem

sudo cp /etc/letsencrypt/live/example.com/fullchain.pem \
    /etc/spooky/certs/fullchain.pem

sudo chown $USER:$USER /etc/spooky/certs/*
sudo chmod 640 /etc/spooky/certs/*
```

### Auto-renewal deploy hook

Create `/etc/letsencrypt/renewal-hooks/deploy/spooky-reload.sh`:

```bash
#!/bin/bash
set -e

DOMAIN="example.com"
SRC="/etc/letsencrypt/live/${DOMAIN}"
DST="/etc/spooky/certs"

cp "${SRC}/fullchain.pem" "${DST}/fullchain.pem"

openssl pkcs8 -topk8 -nocrypt \
  -in "${SRC}/privkey.pem" \
  -out "${DST}/privkey.pem"

chown $SUDO_USER:$SUDO_USER "${DST}"/*.pem
systemctl restart spooky
```

```bash
sudo chmod +x /etc/letsencrypt/renewal-hooks/deploy/spooky-reload.sh
sudo certbot renew --dry-run   # test renewal
```

---

## Option 2: Let's Encrypt with acme.sh (No Port 80 Required)

`acme.sh` supports DNS-01 challenges — no need to stop any service on port 80.

```bash
curl https://get.acme.sh | sh -s email=admin@example.com
source ~/.bashrc

# Issue cert via DNS challenge (requires DNS provider API key)
# Example for Cloudflare:
export CF_Token="your-cloudflare-api-token"
~/.acme.sh/acme.sh --issue --dns dns_cf \
  -d example.com \
  --server letsencrypt

# Install to spooky cert dir with PKCS#8 key conversion
sudo mkdir -p /etc/spooky/certs

~/.acme.sh/acme.sh --install-cert -d example.com \
  --cert-file      /etc/spooky/certs/fullchain.pem \
  --key-file       /tmp/privkey-pkcs1.pem \
  --reloadcmd      "openssl pkcs8 -topk8 -nocrypt -in /tmp/privkey-pkcs1.pem -out /etc/spooky/certs/privkey.pem && systemctl restart spooky"
```

---

## Multi-Domain SNI Certificates

Serve multiple domains from one listener with per-domain cert selection:

```yaml
listen:
  tls:
    cert: /etc/spooky/certs/default-fullchain.pem   # fallback when SNI unmatched
    key:  /etc/spooky/certs/default-privkey.pem
    certificates:
      - server_name: "example.com"
        cert: /etc/spooky/certs/spooky-fullchain.pem
        key:  /etc/spooky/certs/spooky-privkey.pem
      - server_name: "api.example.com"
        cert: /etc/spooky/certs/api-fullchain.pem
        key:  /etc/spooky/certs/api-privkey.pem
```

Certificate selection order:
1. Exact SNI match in `certificates` array
2. Fallback to `cert`/`key` if no match
3. If no `cert`/`key`, falls back to first `certificates` entry

---

## Verifying Your Certificates

```bash
# Check issuer, subject, expiry
openssl x509 -in /etc/spooky/certs/fullchain.pem -noout -issuer -subject -dates

# Verify cert and key match (both lines must print same hash)
openssl x509 -noout -modulus -in /etc/spooky/certs/fullchain.pem | openssl md5
openssl pkey -noout -modulus -in /etc/spooky/certs/privkey.pem   | openssl md5

# Check key format — both PKCS#8 and PKCS#1 PEM keys are accepted
head -1 /etc/spooky/certs/privkey.pem
# PKCS#8:  -----BEGIN PRIVATE KEY-----      (accepted)
# PKCS#1:  -----BEGIN RSA PRIVATE KEY-----  (also accepted)
```

Both key encodings load fine. If you nonetheless want to normalize a PKCS#1 key to PKCS#8:
```bash
openssl pkcs8 -topk8 -nocrypt -in old-privkey.pem -out /etc/spooky/certs/privkey.pem
```

---

## Troubleshooting

| Error | Cause | Fix |
|-------|-------|-----|
| `Cannot open listen.tls.cert` | Wrong path or permissions | `chown ubuntu:ubuntu /etc/spooky/certs/*` |
| `Cannot parse PEM private key` | Key file is malformed or not a PEM private key | Ensure the file is a valid PEM key (PKCS#8 or PKCS#1 both work) |
| `NET::ERR_CERT_AUTHORITY_INVALID` | Self-signed cert | Use Let's Encrypt cert (Options 1–3 above) |
| `NET::ERR_CERT_COMMON_NAME_INVALID` | Cert domain doesn't match | Issue cert for the exact domain being served |
| `HSTS` blocks bypass | Domain has HSTS preloaded | Must use a valid trusted cert — no bypass possible |
