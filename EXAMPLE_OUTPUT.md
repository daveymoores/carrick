# Example CI Mode Output

## Local Mode (Unchanged)
```bash
$ cargo run -- ../test_repos/repo-a/ ../test_repos/repo-b/
---> Analyzing JavaScript/TypeScript files in: ../test_repos/repo-a/
Found 15 files to analyze in directory ../test_repos/repo-a/
---> Analyzing JavaScript/TypeScript files in: ../test_repos/repo-b/
Found 12 files to analyze in directory ../test_repos/repo-b/

API Analysis Results:
=====================
Found 8 endpoints across all files
Found 5 API calls across all files

All types are compatible!

Found 2 API issues:
1. Orphaned endpoint: No call matching endpoint GET /health
2. Orphaned endpoint: No call matching endpoint GET /metrics
```

## CI Mode - First Repository Run
```bash
$ export CI=true
$ export CARRICK_TOKEN="ecommerce-project-123"
$ export MONGODB_URI="mongodb+srv://user:pass@cluster.mongodb.net/"
$ cargo run -- /path/to/user-service

Running Carrick in CI mode with token: ecommerce-project-123
MongoDB connection verified
---> Analyzing JavaScript/TypeScript files in: /path/to/user-service
Found 8 files to analyze in directory /path/to/user-service
Parsing: /path/to/user-service/src/routes/users.js
Parsing: /path/to/user-service/src/routes/orders.js
Parsing: /path/to/user-service/src/app.js
Analyzed current repo: user-service
Processing 12 types from repository: /path/to/user-service
Uploaded current repo data to cloud storage
Downloaded data from 1 repos
Reconstructed analyzer with cross-repo data

API Analysis Results:
=====================
Found 4 endpoints across all files
Found 3 API calls across all files

Found 8 API issues:
1. Missing endpoint: No endpoint defined for GET ENV_VAR:ORDER_SERVICE_URL:/orders
2. Missing endpoint: No endpoint defined for POST ENV_VAR:PAYMENT_SERVICE_URL:/payments
3. Orphaned endpoint: No call matching endpoint GET /users/:id
4. Orphaned endpoint: No call matching endpoint POST /users
5. Orphaned endpoint: No call matching endpoint GET /users/:id/profile
6. Orphaned endpoint: No call matching endpoint DELETE /users/:id
7. Orphaned call: GET /health (external monitoring)
8. Type mismatch on GET /users/:id: Producer (User) incompatible with Consumer (UserProfile)
```

## CI Mode - After Order Service Runs
```bash
# Order service runs with same token
$ export CI=true
$ export CARRICK_TOKEN="ecommerce-project-123"
$ cargo run -- /path/to/order-service

Running Carrick in CI mode with token: ecommerce-project-123
MongoDB connection verified
Analyzed current repo: order-service
Uploaded current repo data to cloud storage
Downloaded data from 2 repos
Reconstructed analyzer with cross-repo data

API Analysis Results:
=====================
Found 7 endpoints across all files
Found 4 API calls across all files

Found 5 API issues:
1. Missing endpoint: No endpoint defined for POST ENV_VAR:PAYMENT_SERVICE_URL:/payments
2. Orphaned endpoint: No call matching endpoint DELETE /users/:id
3. Orphaned endpoint: No call matching endpoint GET /metrics
4. Orphaned call: GET /health (external monitoring)
5. Type mismatch on GET /users/:id: Producer (User) incompatible with Consumer (UserProfile)
```

## CI Mode - After All Services Run
```bash
# Payment service runs with same token
$ export CI=true
$ export CARRICK_TOKEN="ecommerce-project-123"
$ cargo run -- /path/to/payment-service

Running Carrick in CI mode with token: ecommerce-project-123
MongoDB connection verified
Analyzed current repo: payment-service
Uploaded current repo data to cloud storage
Downloaded data from 3 repos
Reconstructed analyzer with cross-repo data

All types are compatible!

Type checking summary:
  Compatible pairs: 8
  Incompatible pairs: 0
  Orphaned producers: 2
  Orphaned consumers: 0

API Analysis Results:
=====================
Found 12 endpoints across all files
Found 10 API calls across all files

Found 3 API issues:
1. Orphaned endpoint: No call matching endpoint DELETE /users/:id
2. Orphaned endpoint: No call matching endpoint GET /metrics
3. Orphaned endpoint: No call matching endpoint GET /health
```

## Expected Progression
- **Single repo**: Many orphaned endpoints/calls and missing endpoints
- **Multiple repos**: Issues resolve as cross-repo connections are established
- **All repos**: Only intentional orphans remain (health checks, admin endpoints, etc.)

## Error Cases
```bash
# Missing MONGODB_URI
$ export CI=true
$ export CARRICK_TOKEN="test"
$ cargo run -- /path/to/project
CI mode failed: Connection error: MONGODB_URI environment variable not set

# Missing CARRICK_TOKEN
$ export CI=true
$ export MONGODB_URI="mongodb://localhost:27017"
$ cargo run -- /path/to/project
CI mode failed: CARRICK_TOKEN must be set in CI mode

# MongoDB connection failure
$ export CI=true
$ export CARRICK_TOKEN="test"
$ export MONGODB_URI="mongodb://invalid:27017"
$ cargo run -- /path/to/project
CI mode failed: Connection error: Failed to connect to MongoDB: ...
```