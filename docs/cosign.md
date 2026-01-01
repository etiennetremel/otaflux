# Firmware Signing with Cosign

OtaFlux supports firmware image signing using Cosign, a tool from the Sigstore
ecosystem designed to sign and verify container images and other OCI artifacts.

## Why This Matters

Distributing firmware over-the-air introduces risks. A compromised registry, a
man-in-the-middle attack, or accidental deployment of an unverified build can
all lead to bricked devices or worse: remote code execution. Firmware
authenticity must be verifiable before an update is accepted.

Cosign provides:

- **Cryptographic integrity checks** using signatures tied to a specific image
  digest
- **Optional transparency logs** via Rekor, which publicly record signing events
  for later auditing
- **Standard tooling** that works with any OCI-compliant registry (Harbor, GitHub
  Container Registry, etc.)

**OtaFlux verifies signatures before serving firmware to a device, so unsigned
or tampered images are automatically rejected.**

## Prerequisites

You will need:

- [Cosign CLI][cosign-cli]
- [ORAS CLI][oras] (for pushing firmware artifacts)

## Generate Key Pair

Generate a Cosign key pair for signing firmware images:

```bash
# Generate key pair (creates cosign.pub and cosign.key)
# You can leave the password empty for automation
cosign generate-key-pair
```

> **Important**: Keep both keys in a secure environment (e.g., KMS, HSM, or
> encrypted storage). The private key should only be accessible to your CI/CD
> pipeline.

## Build and Push Firmware

Build your firmware image and push it to an OCI registry. Below is an example
using [espflash][espflash] for ESP32-based devices:

```bash
# Required for ESP workflow
. $HOME/export-esp.sh

# Compile project
cargo build --release

# Save as binary image
espflash save-image \
    --chip esp32 \
    ./target/xtensa-esp32-none-elf/release/my-device \
    ./firmware.bin

# Push the firmware binary to an OCI registry
oras push "registry.example.com:443/my-project/my-device:0.1.2" \
    firmware.bin:application/vnd.espressif.esp32.firmware.v1+binary
```

## Sign the Artifact

Sign the artifact using Cosign (you need the sha256 digest of the pushed image):

```bash
# Sign the artifact
cosign sign --key cosign.key \
    registry.example.com:443/my-project/my-device@sha256:<digest>

# Verify the signature (optional but recommended)
cosign verify --key cosign.pub \
    registry.example.com:443/my-project/my-device@sha256:<digest>
```

## Configure OtaFlux

Pass the public key to OtaFlux to enable signature verification:

```bash
podman run -ti --rm \
    -v $PWD/cosign.pub:/etc/otaflux/cosign.pub:ro \
    -p 8080:8080 \
    -p 9090:9090 \
    ghcr.io/etiennetremel/otaflux \
        --log-level "debug" \
        --registry-url "https://your-registry.example.com" \
        --repository-prefix "my-project/" \
        --registry-username "username" \
        --registry-password "password" \
        --cosign-pub-key-path "/etc/otaflux/cosign.pub"
```

## Test Verification

```bash
curl 'localhost:8080/version?device=my-device'
0.1.2
2258256831
953968
```

If the signature verification fails, OtaFlux returns an error and refuses to
serve the firmware.

## CI/CD Integration

Example GitHub Actions workflow for automated signing:

```yaml
name: Build and Sign Firmware

on:
  push:
    tags:
      - 'v*'

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      
      - name: Build firmware
        run: |
          # Your build commands here
          
      - name: Install Cosign
        uses: sigstore/cosign-installer@v3
        
      - name: Install ORAS
        uses: oras-project/setup-oras@v1
        
      - name: Push firmware
        run: |
          oras push "${{ vars.REGISTRY }}/my-device:${{ github.ref_name }}" \
            firmware.bin:application/vnd.espressif.esp32.firmware.v1+binary
            
      - name: Sign firmware
        env:
          COSIGN_KEY: ${{ secrets.COSIGN_PRIVATE_KEY }}
        run: |
          echo "$COSIGN_KEY" > cosign.key
          DIGEST=$(oras manifest fetch "${{ vars.REGISTRY }}/my-device:${{ github.ref_name }}" --descriptor | jq -r '.digest')
          cosign sign --key cosign.key "${{ vars.REGISTRY }}/my-device@${DIGEST}"
```

<!-- page links -->
[cosign-cli]: https://docs.sigstore.dev/cosign/system_config/installation/
[espflash]: https://github.com/esp-rs/espflash
[oras]: https://oras.land
