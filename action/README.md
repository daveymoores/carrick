# Carrick GitHub Action

Analyze JavaScript/TypeScript APIs for cross-repository inconsistencies.

## Usage

```yaml
- name: Run Carrick Analysis
  uses: daveymoores/carrick@v1
  with:
    carrick-org: ${{ secrets.CARRICK_ORG }}
    carrick-api-key: ${{ secrets.CARRICK_API_KEY }}
    carrick-lambda-url: ${{ secrets.CARRICK_LAMBDA_URL }}
```

## Inputs

- `path` - Path to analyze (default: `.`)
- `carrick-org` - Organization name (required)
- `carrick-api-key` - API key (required)
- `carrick-lambda-url` - Lambda URL (required)

## Outputs

- `success` - Analysis completed successfully
- `issues-found` - Number of API issues detected

## Setup

Add these secrets to your repository:
- `CARRICK_ORG`
- `CARRICK_API_KEY`  
- `CARRICK_LAMBDA_URL`