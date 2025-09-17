# Generate Certificate and Keys for SSL

## Generate a Self-Signed Certificate (Development)

Run this in your shell (Linux/Mac, or Git Bash on Windows):

```sh
# Private key (PKCS#8 DER, no password)
openssl genpkey -algorithm RSA -out key.pem -pkeyopt rsa_keygen_bits:2048
openssl pkcs8 -topk8 -inform PEM -outform DER -in key.pem -out key.der -nocrypt

# Certificate (DER)
openssl req -new -x509 -key key.pem -out cert.der -outform DER -days 365 -subj "/CN=localhost"
```
