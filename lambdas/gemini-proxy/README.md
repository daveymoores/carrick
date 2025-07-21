# Carrick Gemini Proxy

A Lambda proxy for secure Google Gemini API access used by the Carrick code analysis tool.

## Overview

This proxy prevents API key exposure in public releases while providing usage controls.

## API Endpoint

### POST /gemini/chat

Requires authentication via `Authorization: Bearer <carrick-api-key>` header.

**Request:**
```json
{
  "messages": [
    {"role": "system", "content": "You are a helpful assistant."},
    {"role": "user", "content": "Analyze this code..."}
  ]
}
```

**Response:**
```json
{
  "success": true,
  "text": "Response from Gemini API...",
  "responseTime": 1250
}
```

## Usage Limits

- **2000 requests per day** across all users
- **1MB maximum** request size
- Resets at midnight UTC

## Environment Variables

| Variable | Description |
|----------|-------------|
| `GEMINI_API_KEY` | Google Gemini API key |
| `VALID_API_KEYS` | Comma-separated Carrick API keys |

## Testing

```bash
npm test
```

## Example Request

```bash
curl -X POST https://your-endpoint/gemini/chat \
  -H "Authorization: Bearer your-carrick-api-key" \
  -H "Content-Type: application/json" \
  -d '{"messages":[{"role":"user","content":"Hello"}]}'
```
