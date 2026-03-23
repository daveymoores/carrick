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

# Build mcp-server function
echo "Building mcp-server..."
cd ../mcp-server
npm install
npm run build:lambda
cd ../lambdas
mkdir -p mcp-server-staging
cp ../mcp-server/dist-lambda/index.js mcp-server-staging/
cd mcp-server-staging && zip -r ../mcp-server.zip . && cd ..
rm -rf mcp-server-staging

echo "Lambda functions built successfully:"
ls -la *.zip

echo ""
echo "To deploy with Terraform:"
echo "1. cd ../terraform"
echo "2. terraform plan"
echo "3. terraform apply"
