#!/bin/bash
set -e

# Configuration
REPO_DIR="apt-repo"
DIST="stable"
COMPONENT="main"
ARCHITECTURES="amd64 arm64"

# Create repo structure
mkdir -p ${REPO_DIR}/pool/main
mkdir -p ${REPO_DIR}/dists/${DIST}/${COMPONENT}/binary-{amd64,arm64}

# Copy .deb files to pool
cp target/debian/*.deb ${REPO_DIR}/pool/main/

# Generate Packages files for each architecture
for arch in ${ARCHITECTURES}; do
    cd ${REPO_DIR}
    dpkg-scanpackages --arch ${arch} pool/ > dists/${DIST}/${COMPONENT}/binary-${arch}/Packages
    gzip -kf dists/${DIST}/${COMPONENT}/binary-${arch}/Packages
    cd -
done

# Generate Release file
cd ${REPO_DIR}/dists/${DIST}
cat > Release <<EOF
Origin: cfcli
Label: cfcli
Suite: ${DIST}
Codename: ${DIST}
Architectures: ${ARCHITECTURES}
Components: ${COMPONENT}
Description: Crazyflie CLI tool repository
Date: $(date -Ru)
EOF

# Add file hashes to Release
apt-ftparchive release . >> Release

# Sign the Release file
gpg --default-key ${GPG_KEY_ID} -abs -o Release.gpg Release
gpg --default-key ${GPG_KEY_ID} --clearsign -o InRelease Release

cd ../../..