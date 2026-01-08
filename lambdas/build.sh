#!/bin/bash

set -e

echo "Building Lambda functions..."

# Clean up any existing zip files
rm -f *.zip

# Build check-or-upload function
echo "Building check-or-upload..."
cd check-or-upload
npm install --production
zip -r ../check-or-upload.zip . -x "*.git*" "node_modules/.cache/*"
cd ..

# Build agent-proxy function
echo "Building agent-proxy..."
cd agent-proxy
npm install --production
zip -r ../agent-proxy.zip . -x "*.git*" "node_modules/.cache/*"
cd ..

echo "Lambda functions built successfully:"
ls -la *.zip

echo ""
echo "To deploy with Terraform:"
echo "1. cd ../terraform"
echo "2. terraform plan"
echo "3. terraform apply"
