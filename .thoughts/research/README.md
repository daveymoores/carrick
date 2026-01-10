# Research Documentation

This directory contains detailed reference documentation for the Carrick project.

**For the project overview and current status, see [../README.md](../README.md)**

---

## Available Documents

### 1. [Cloud Infrastructure](./cloud_infrastructure.md)

**Purpose**: Complete architecture documentation for AWS cloud integration

**Contents**:
- AWS services architecture (S3, DynamoDB, Lambda, API Gateway)
- Upload/download workflows
- Multi-repo data flow
- Caching and optimization strategies
- Security architecture
- Cost optimization

**When to read**: Understanding cloud storage, debugging AWS issues, planning infrastructure changes

---

### 2. [TypeScript Type Checking System](./ts_check.md)

**Purpose**: Documentation of the `ts_check` TypeScript type compatibility system

**Contents**:
- Type extraction architecture
- Type compatibility checking
- Producer/consumer type validation
- ts-morph integration
- Workflow and algorithms

**When to read**: Understanding type mismatch detection, debugging type checking, modifying the type system

---

### 3. [Testing Strategy](./testing_strategy.md)

**Purpose**: Comprehensive testing strategy and coverage documentation

**Contents**:
- Testing philosophy (output-focused)
- Test coverage summary (70+ tests)
- Test architecture (unit, integration, output contract)
- What is tested vs not tested
- Running tests
- CI/CD integration
- Adding new tests

**When to read**: Before refactoring, adding new tests, understanding test failures

---

## Quick Navigation

| I want to... | Read this |
|--------------|-----------|
| Understand project status | [../README.md](../README.md) |
| Understand AWS integration | [cloud_infrastructure.md](./cloud_infrastructure.md) |
| Debug type checking | [ts_check.md](./ts_check.md) |
| Add or understand tests | [testing_strategy.md](./testing_strategy.md) |

---

## Document Status

| Document | Status | Last Updated |
|----------|--------|--------------|
| cloud_infrastructure.md | ✅ Current | January 2025 |
| ts_check.md | ✅ Current | January 2025 |
| testing_strategy.md | ✅ Current | January 2025 |