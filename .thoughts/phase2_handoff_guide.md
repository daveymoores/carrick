# Phase 2 Handoff Guide: Multi-Agent Migration

**Status**: Phase 2 Priority 1 Complete (60% of Phase 2)  
**Date**: January 2025  
**Next Task**: Complete Priority 2 & 3 (5-7 hours estimated)

---

## 🎯 What You Need to Know

### TL;DR
- ✅ **Priority 1 DONE**: Adapter layer removed, CloudRepoData built directly from multi-agent results
- 🔴 **Priority 2 TODO**: Refactor `get_results()` to use mount graph, remove legacy analysis methods
- 🔴 **Priority 3 TODO**: Simplify DependencyVisitor to only extract imports/functions
- ✅ **All 46 tests passing**, clippy clean, multi-framework validated

### Current State
```
MultiAgentOrchestrator → CloudRepoData::from_multi_agent_results() → CloudRepoData
                              ↓
                        (saved to cloud storage)
                              ↓
                     Cross-Repo Analysis
                              ↓
                  Analyzer::get_results() ← STILL USES LEGACY METHODS
```

**The Problem**: `get_results()` calls legacy methods that should use mount graph instead.

---

## 📚 Documentation Roadmap

### Start Here (Read in This Order)

1. **[PHASE_2_SUMMARY.md](PHASE_2_SUMMARY.md)** ⏱️ 2 min
   - What was accomplished in Priority 1
   - Quick status overview
   - Test results

2. **[research/MIGRATION_STATUS.md](research/MIGRATION_STATUS.md)** ⏱️ 15 min
   - Complete project status (all phases)
   - What's working, what's remaining
   - Architecture comparison (before/after)

3. **[research/PHASE_2_REMAINING_WORK.md](research/PHASE_2_REMAINING_WORK.md)** ⏱️ 30 min
   - **YOUR IMPLEMENTATION GUIDE** (step-by-step instructions)
   - Code examples and pseudocode
   - Testing strategy and troubleshooting
   - **Read this before writing any code**

### Additional Context

4. **[research/PHASE_2_COMPLETE.md](research/PHASE_2_COMPLETE.md)** ⏱️ 10 min
   - Detailed report on what was done in Priority 1
   - Technical implementation details
   - Architecture diagrams

5. **[.thoughts/multi_agent_framework_agnostic_analysis.md](.thoughts/multi_agent_framework_agnostic_analysis.md)** ⏱️ 60 min
   - Original architecture vision
   - Deep dive into the "why"
   - Reference for design decisions

6. **[research/README.md](research/README.md)** ⏱️ 5 min
   - Documentation index
   - Navigation guide
   - Quick reference

---

## 🚀 Quick Start (30 Minutes to Productive)

### Step 1: Verify Everything Works (5 min)
```bash
cd carrick
export CARRICK_API_ENDPOINT=http://localhost:8000

# Run all tests (should see 46/46 passing)
cargo test

# Check code quality
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

**Expected Output**: All tests pass, no warnings

### Step 2: Read Documentation (25 min)
1. PHASE_2_SUMMARY.md (2 min)
2. research/MIGRATION_STATUS.md (15 min) - Focus on:
   - Lines 135-245: What still exists
   - Lines 354-395: Remaining work
   - Lines 562-590: Architecture comparison
3. research/PHASE_2_REMAINING_WORK.md (8 min) - Skim to understand scope

### Step 3: Understand the Code (20 min)
```bash
# Look at the mount graph API (your replacement for legacy methods)
grep -A 10 "pub fn find_matching_endpoints" src/mount_graph.rs

# Look at the legacy methods you'll replace
grep -A 20 "pub fn analyze_matches" src/analyzer/mod.rs
grep -A 20 "pub fn compare_calls_to_endpoints" src/analyzer/mod.rs

# Look at where they're called
grep -A 10 "pub fn get_results" src/analyzer/mod.rs
```

---

## 🎯 Your Mission: Complete Phase 2

### Priority 2: Remove Legacy Analysis Methods (2-3 hours)

**Goal**: Make `Analyzer::get_results()` use mount graph instead of legacy pattern matching

**What to Change**:
1. Add `mount_graph: Option<MountGraph>` field to `CloudRepoData` struct
2. Add `mount_graph: Option<MountGraph>` field to `Analyzer` struct
3. Update `CloudRepoData::from_multi_agent_results()` to include mount graph
4. Create `MountGraph::merge_from_repos()` for cross-repo analysis
5. Implement `Analyzer::analyze_matches_with_mount_graph()`
6. Implement `Analyzer::compare_calls_with_mount_graph()`
7. Update `Analyzer::get_results()` to call new methods
8. Delete legacy methods once tests pass

**Files to Modify**:
- `src/cloud_storage/mod.rs` (add mount_graph field)
- `src/analyzer/mod.rs` (add field, new methods, delete old methods)
- `src/mount_graph.rs` (add merge method)
- `src/engine/mod.rs` (pass mount graph through)

**Detailed Guide**: See `research/PHASE_2_REMAINING_WORK.md` lines 102-536

### Priority 3: Simplify DependencyVisitor (3-4 hours)

**Goal**: Extract only imports and functions, not endpoints/calls/mounts

**What to Change**:
1. Create new `src/symbol_extractor.rs` with only import/function extraction
2. Update `src/engine/mod.rs` to use `SymbolExtractor` instead of `DependencyVisitor`
3. Remove or archive `src/visitor.rs` (optional - can keep type definitions)

**Files to Create/Modify**:
- `src/symbol_extractor.rs` (new file)
- `src/engine/mod.rs` (update usage)
- `src/lib.rs` (add module)

**Detailed Guide**: See `research/PHASE_2_REMAINING_WORK.md` lines 538-636

---

## 🧪 Testing Strategy

### After Each Change
```bash
# Run tests
CARRICK_API_ENDPOINT=http://localhost:8000 cargo test

# Check for warnings
CARRICK_API_ENDPOINT=http://localhost:8000 cargo clippy --all-targets -- -D warnings

# Format code
cargo fmt
```

### Key Tests to Watch
- **Integration tests** (3 tests): Must pass - these validate the whole system
- **Mount graph tests** (3 tests): Verify mount graph logic works
- **Endpoint matching tests** (10 tests): May need updating or removal after Priority 2

### When to Commit
```bash
# After Priority 2 (legacy methods removed)
git add .
git commit -m "Phase 2 (Priority 2): Refactor get_results() to use mount graph, remove legacy analysis methods"

# After Priority 3 (visitor simplified)
git add .
git commit -m "Phase 2 (Priority 3): Create SymbolExtractor, remove endpoint/call extraction from visitor"

# After everything passes
git commit -m "Phase 2 Complete: All legacy analysis code removed"
```

---

## 🗺️ Code Architecture Map

### Current Architecture (Post-Priority 1)
```
analyze_current_repo() in src/engine/mod.rs
    ↓
MultiAgentOrchestrator::run_complete_analysis()
    ↓
MultiAgentAnalysisResult (contains MountGraph)
    ↓
CloudRepoData::from_multi_agent_results() ← NEW in Priority 1
    ↓
CloudRepoData (with endpoints, calls, mounts from mount graph)
    ↓
[Serialized to cloud storage]
    ↓
run_analysis_engine() downloads all repos
    ↓
build_cross_repo_analyzer() merges data
    ↓
Analyzer::get_results() ← USES LEGACY METHODS (Priority 2 will fix)
    ↓
print_results()
```

### Target Architecture (After Priority 2)
```
analyze_current_repo()
    ↓
MultiAgentOrchestrator::run_complete_analysis()
    ↓
MultiAgentAnalysisResult (contains MountGraph)
    ↓
CloudRepoData::from_multi_agent_results() (includes mount_graph)
    ↓
CloudRepoData (with mount_graph field)
    ↓
[Serialized to cloud storage]
    ↓
run_analysis_engine() downloads all repos
    ↓
build_cross_repo_analyzer() merges data + mount graphs
    ↓
Analyzer::get_results() → uses mount_graph methods ← FIXED
    ↓
print_results()
```

---

## 🔍 Key Files Reference

### Files You'll Modify

| File | Purpose | Priority 2 | Priority 3 |
|------|---------|-----------|-----------|
| `src/cloud_storage/mod.rs` | CloudRepoData struct | Add mount_graph field | - |
| `src/analyzer/mod.rs` | Analyzer logic | Add mount_graph, new methods, delete legacy | - |
| `src/mount_graph.rs` | Mount graph logic | Add merge method | - |
| `src/engine/mod.rs` | Analysis orchestration | Pass mount graph | Use SymbolExtractor |
| `src/symbol_extractor.rs` | Symbol extraction | - | Create new file |
| `src/lib.rs` | Module declarations | - | Add symbol_extractor |

### Files to Study (Don't Modify)

- `src/multi_agent_orchestrator.rs` - Multi-agent workflow (already works)
- `tests/integration_test.rs` - Integration tests (must continue to pass)
- `tests/multi_agent_test.rs` - Multi-agent tests (must continue to pass)

### Legacy Files (Will Delete/Update)

- `tests/endpoint_matching_test.rs` - Tests legacy methods (update or remove in Priority 2)
- `src/visitor.rs` - DependencyVisitor (simplify or replace in Priority 3)

---

## 💡 Key Insights

### Why Mount Graph is Better
- **Framework agnostic**: Uses behavior-based classification, not Express patterns
- **Already built**: No runtime pattern matching needed
- **Single source of truth**: All endpoint/call relationships in one place
- **Path resolution**: Handles mount prefixes automatically

### Why We Can't Just Delete Legacy Methods
They're still called by `Analyzer::get_results()` which is used in cross-repo analysis. Once we refactor `get_results()` to use mount graph, we can safely delete them.

### The Adapter Layer Problem (Now Solved)
Before Priority 1, we were converting multi-agent results back to legacy format, then to CloudRepoData. Now we go directly from multi-agent to CloudRepoData. This eliminated ~60 lines of conversion code.

---

## 🚨 Common Pitfalls

### 1. Mount Graph Not Serializing
**Symptom**: Tests fail with serialization errors

**Solution**: Ensure all MountGraph types derive `Serialize, Deserialize`
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountGraph { ... }
```

### 2. Tests Timing Out
**Symptom**: Tests hang or timeout

**Solution**: Check for circular references in mount graph merge logic

### 3. Different Results Than Legacy
**Symptom**: Tests expect different output

**Solution**: This might be correct! Mount graph is more accurate. Update tests if needed.

### 4. Clippy Warnings About Dead Code
**Symptom**: Warnings about unused fields/methods

**Solution**: Add `#[allow(dead_code)]` temporarily during migration

---

## 📊 Success Metrics

### You're Done When:
- [ ] All tests pass (estimated 40-45 after cleanup)
- [ ] No clippy warnings
- [ ] `analyze_matches()` deleted
- [ ] `compare_calls_to_endpoints()` deleted
- [ ] `find_matching_endpoint()` deleted
- [ ] `DependencyVisitor` simplified or replaced
- [ ] Cross-repo analysis uses mount graph
- [ ] Documentation updated

### How to Verify:
```bash
# All tests pass
CARRICK_API_ENDPOINT=http://localhost:8000 cargo test

# No warnings
CARRICK_API_ENDPOINT=http://localhost:8000 cargo clippy --all-targets -- -D warnings

# Code formatted
cargo fmt --check

# Legacy methods gone
! grep -q "pub fn analyze_matches" src/analyzer/mod.rs
! grep -q "pub fn compare_calls_to_endpoints" src/analyzer/mod.rs
```

---

## 🆘 If You Get Stuck

### Debugging Steps
1. Add `println!` statements to see data flow
2. Run single test: `cargo test test_name -- --nocapture`
3. Check git history: `git log --oneline --all --graph`
4. Read the mount graph tests to see how it's used

### Reference Implementations
- Mount graph usage: `tests/multi_agent_test.rs:test_mount_graph_construction`
- Cross-repo logic: `src/engine/mod.rs:build_cross_repo_analyzer`
- Serialization: `tests/mock_storage_test.rs`

### Questions to Ask
1. Is the mount graph being passed through correctly?
2. Does CloudRepoData have the mount_graph field?
3. Is the mount graph being merged in cross-repo mode?
4. Are the new methods being called instead of legacy ones?

---

## 📝 Documentation Updates Needed

After completing work, update:
1. **research/MIGRATION_STATUS.md**
   - Change Phase 2 status to "✅ Complete"
   - Update "Remaining Work" section
   - Update metrics

2. **PHASE_2_SUMMARY.md**
   - Add Priority 2 & 3 completion
   - Update status table

3. **research/PHASE_2_COMPLETE.md**
   - Add sections for Priority 2 & 3
   - Document what was changed

4. **This file (HANDOFF_GUIDE.md)**
   - Add "✅ COMPLETE" at top
   - Archive for future reference

---

## 🎓 Learning Resources

### Understanding the System
- Read integration tests: They show how everything should work together
- Study mount graph: `src/mount_graph.rs` - this is your replacement
- Check git history: See how Phase 0 and 1 were completed

### Rust Help
- Serialization: https://serde.rs/
- Error handling: Result<T, E> pattern is used throughout
- Testing: https://doc.rust-lang.org/book/ch11-00-testing.html

---

## ⏱️ Time Estimates

| Task | Time | Difficulty |
|------|------|-----------|
| Read documentation | 45 min | Easy |
| Verify tests pass | 5 min | Easy |
| Add mount_graph fields | 30 min | Easy |
| Implement new methods | 1 hour | Medium |
| Update cross-repo logic | 1 hour | Medium |
| Test and debug | 1 hour | Medium |
| Remove legacy methods | 30 min | Easy |
| Create SymbolExtractor | 2 hours | Medium |
| Final testing | 1 hour | Easy |
| Update documentation | 30 min | Easy |
| **TOTAL** | **8-9 hours** | **Medium** |

---

## ✅ Pre-Flight Checklist

Before you start coding:
- [ ] Read PHASE_2_SUMMARY.md
- [ ] Read research/MIGRATION_STATUS.md
- [ ] Read research/PHASE_2_REMAINING_WORK.md (at least skim)
- [ ] Verified all tests pass on current code
- [ ] Understand what mount graph does
- [ ] Know where legacy methods are called
- [ ] Have a plan for Priority 2
- [ ] Have a plan for Priority 3

Before you commit:
- [ ] All tests pass
- [ ] No clippy warnings
- [ ] Code formatted
- [ ] Documentation updated
- [ ] Commit message is clear

---

## 🎉 Final Notes

**You've got this!** Phase 0 and 1 were the hard parts (fixing bugs, extracting types). Phase 2 is just cleaning up legacy code that's already been replaced by better implementations.

**Key mindset**: You're not building something new. You're replacing old methods with mount graph queries. The mount graph already works and is tested.

**Remember**: Test after every change. Small commits. Read the guide. Ask questions (add code comments).

**When you're done**: The system will be fully framework-agnostic with no legacy pattern matching. Clean architecture, single source of truth, and ready for any framework.

Good luck! 🚀

---

**Questions?** Read the docs above, check the code comments, or review test files.

**Stuck?** Add debug logging, run tests one at a time, check git history.

**Success?** Update the docs and celebrate! 🎊