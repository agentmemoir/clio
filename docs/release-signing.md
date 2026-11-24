# Release signing

Each release publishes `checksums.txt` beside the archives and `install.sh`, plus `checksums.txt.sig`, a detached RSA signature over `checksums.txt` made with the release signing key. The installer carries the public half of that key and verifies the signature before it downloads an archive, so a swapped archive or a swapped manifest is refused.

The public key in `scripts/install.sh` has the SHA-256 fingerprint `d7ff9d436841ec7203e597d998f6e3425e2e959c831262c2d7b391ce11967a1c`, computed over its DER SubjectPublicKeyInfo: `openssl pkey -pubin -in install-key.pub.pem -outform DER | sha256sum`. The `release.yml` signing step pins the same value and fails the release when the key in the published installer hashes to anything else.

The release owner holds the private half outside this repository, and it is not committed, copied, or printed anywhere. It reaches CI only through the `CLIO_RELEASE_SIGNING_KEY` repository secret.

To replace the keypair, generate a 3072-bit RSA key with `umask 077; openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out release-signing-key.pem`, write the public half with `openssl pkey -in release-signing-key.pem -pubout -out release-signing-key.pub.pem`, put the public half in `scripts/install.sh` in place of the current block, and update the pinned fingerprint in `release.yml` with `openssl pkey -pubin -in release-signing-key.pub.pem -outform DER | sha256sum`.

Store the private half with `gh secret set CLIO_RELEASE_SIGNING_KEY < release-signing-key.pem` in the `agentmemoir/clio` repository, then remove the local copy when it is no longer needed.

The limit is the channel: the install command fetches `install.sh` from the same release as the manifest, and the public key travels inside that file. The signature defeats a swapped archive or manifest and a wrong or stale key in the secret, but an attacker who can replace the installer replaces the key with it. Pinning the key outside the release channel, such as a package manager or a published fingerprint, is what would close that gap.
