# Research Documentation

This directory contains research and analysis documents for the Carrick project.

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

**When to read**:
- Understanding how cloud storage works
- Debugging AWS integration issues
- Planning infrastructure changes

---

### 2. [TypeScript Type Checking System](./ts_check.md)
**Purpose**: Documentation of the `ts_check` TypeScript type compatibility system

**Contents**:
- Type extraction architecture
- Type compatibility checking
- Producer/consumer type validation
- ts-morph integration
- Workflow and algorithms

**When to read**:
- Understanding type mismatch detection
- Debugging type checking issues
- Planning type system changes

---

### 3. [Testing Strategy](./testing_strategy.md) ⭐ **NEW**
**Purpose**: Comprehensive testing strategy and coverage documentation

**Contents**:
- Testing philosophy (output-focused)
- Test coverage summary (43 tests)
- Test architecture (unit, integration, output contract)
- What is tested vs not tested
- Running tests
- CI/CD integration
- Adding new tests

**When to read**:
- Before refactoring (understand test coverage)
- Adding new tests
- Understanding test failures
- CI/CD debugging

---

## Related Documentation

### In `.thoughts/` Directory

**Test Coverage Implementation**:
- `test-coverage-complete.md` - Full implementation report
- `test-coverage-progress.md` - Initial progress tracking
- `adding-output-tests-guide.md` - Step-by-step guide for adding tests
- `test-implementation-summary.md` - Quick summary

**Project State**:
- `project_state_2025.md` - Overall project architecture and state

---

## Quick Navigation

### I want to...

**Understand how tests work**
→ Read `testing_strategy.md`

**Add a new test**
→ Read `../adding-output-tests-guide.md`

**Understand AWS integration**
→ Read `cloud_infrastructure.md`

**Debug type checking**
→ Read `ts_check.md`

**See test implementation details**
→ Read `../test-coverage-complete.md`

**Get a quick test overview**
→ Read `../test-implementation-summary.md`

---

## Document Status

| Document | Status | Last Updated |
|----------|--------|--------------|
| cloud_infrastructure.md | ✅ Current | 2025 |
| ts_check.md | ✅ Current | 2025 |
| testing_strategy.md | ✅ Current | 2025-11-15 |

---

## Contributing

When adding new research documents:

1. Place them in `.thoughts/research/`
2. Update this README with a summary
3. Link to related documents
4. Update the "Document Status" table
5. Add to "Quick Navigation" if applicable

---

## Document Maintenance

### When to Update

- **cloud_infrastructure.md**: When AWS architecture changes
- **ts_check.md**: When type checking system changes
- **testing_strategy.md**: When tests are added/removed or strategy changes

### Review Schedule

- Review quarterly for accuracy
- Update after major refactoring
- Update after infrastructure changes
