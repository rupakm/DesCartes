# Design Document

## Overview

The DESCARTES User Manual will be implemented as an mdbook that provides comprehensive documentation for the discrete event simulation framework. The manual follows a progressive learning approach, starting with basic concepts and building up to advanced usage patterns. The design emphasizes practical examples, clear explanations, and modular organization to serve both newcomers and experienced users.

## Architecture

### File Structure

The manual will be organized as a standard mdbook project with the following structure:

```
doc/mdbook/
├── book.toml                 # mdbook configuration
├── src/
│   ├── SUMMARY.md           # Table of contents
│   ├── introduction.md      # Brief introduction
│   ├── chapter_01/
│   │   ├── README.md        # Chapter 1: Basics of DES Simulation
│   │   ├── quick_start.md   # 1.1 Quick Start
│   │   ├── des_basics.md    # 1.2 Basics of Discrete Event Simulation
│   │   └── metrics_overview.md # 1.3 The Metrics Collection System
│   ├── chapter_02/
│   │   └── README.md        # Chapter 2: DES-Core: The Core Abstractions
│   ├── chapter_03/
│   │   └── README.md        # Chapter 3: DES-Components: Components for Server Modeling
│   ├── chapter_04/
│   │   └── README.md        # Chapter 4: Metrics Collection
│   ├── chapter_05/
│   │   ├── README.md        # Chapter 5: Example: M/M/k Queues
│   │   ├── basic_queues.md  # 5.1 Basic M/M/1 and M/M/k Queues
│   │   ├── retry_policies.md # 5.2 Clients with timeouts and retry policies
│   │   └── admission_control.md # 5.3 Clients with admission control
│   ├── chapter_06/
│   │   ├── README.md        # Chapter 6: Tower and Rust Server Infrastructure
│   │   ├── abstractions.md  # 6.1 Common abstractions
│   │   ├── examples.md      # 6.2 Small examples of tower servers
│   │   └── multi_tier.md    # 6.3 Multiple servers (App -> DB, LB -> App -> DB)
│   ├── chapter_07/
│   │   ├── README.md        # Chapter 7: Advanced examples
│   │   ├── payload_service_times.md # 7.1 Service time distributions that depend on payload
│   │   ├── client_admission_control.md # 7.2 Client-side admission control
│   │   ├── server_throttling.md # 7.3 Server-side throttling and admission control
│   │   └── workload_generation.md # 7.4 Advanced workload generation patterns
│   └── assets/
│       ├── images/          # Diagrams and screenshots
│       ├── code/           # Complete code examples
│       └── css/            # Custom styling
```

### Content Organization Strategy

The manual follows a **progressive disclosure** approach:

1. **Foundation First**: Core concepts before implementation details
2. **Example-Driven**: Every concept illustrated with working code
3. **Modular Sections**: Each chapter can be read independently after Chapter 1
4. **Cross-References**: Links between related concepts across chapters
5. **Practical Focus**: Real-world scenarios over theoretical discussions

## Components and Interfaces

### Chapter Structure Template

Each chapter follows a consistent structure:

```markdown
# Chapter Title

## Overview
Brief description of what this chapter covers and why it matters.

## Prerequisites
What the reader should know before reading this chapter.

## Key Concepts
Core ideas introduced in this chapter.

## Examples
Working code examples with explanations.

## Common Patterns
Reusable patterns and best practices.

## Troubleshooting
Common issues and solutions.

## Next Steps
What to read next and how this connects to other chapters.
```

### Code Example Standards

All code examples must:

- **Compile and Run**: Every example is tested in CI
- **Self-Contained**: Include all necessary imports and setup
- **Commented**: Explain non-obvious parts
- **Realistic**: Use meaningful variable names and scenarios
- **Progressive**: Build on previous examples where appropriate

### Cross-Reference System

The manual uses a consistent cross-referencing system:

- **Internal Links**: `[Component trait](../chapter_02/README.md#component-trait)`
- **API References**: `[Simulation::add_component](https://docs.rs/des-core/latest/des_core/struct.Simulation.html#method.add_component)`
- **Example References**: `[M/M/1 Queue Example](../chapter_05/basic_queues.md#mm1-implementation)`

## Data Models

### Configuration Model

The `book.toml` configuration includes:

```toml
[book]
title = "DESCARTES User Manual"
description = "Comprehensive guide to the DESCARTES discrete event simulation framework"
authors = ["DESCARTES Contributors"]
language = "en"
multilingual = false
src = "src"

[build]
build-dir = "book"
create-missing = true

[preprocessor.links]
# Enable link checking

[output.html]
default-theme = "light"
preferred-dark-theme = "navy"
curly-quotes = true
mathjax-support = true
copy-fonts = true
additional-css = ["assets/css/custom.css"]

[output.html.search]
enable = true
limit-results = 30
teaser-word-count = 30
use-boolean-and = true
boost-title = 2
boost-hierarchy = 1
boost-paragraph = 1
expand = true
heading-split-level = 3

[output.html.playground]
runnable = true
```

### Content Metadata

Each chapter includes frontmatter for metadata:

```yaml
---
title: "Chapter Title"
weight: 10
description: "Brief chapter description"
keywords: ["simulation", "rust", "des"]
---
```

### Example Code Structure

Code examples follow this template:

```rust
//! Example: [Brief Description]
//!
//! This example demonstrates [key concepts].
//!
//! Run with: cargo run --example example_name

use des_core::{Simulation, SimTime, Component, Key};
use des_components::{Server, SimpleClient};
use std::time::Duration;

// [Example implementation with detailed comments]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // [Setup and execution]
    Ok(())
}
```

Now I need to use the prework tool to analyze the acceptance criteria before writing the correctness properties:

<function_calls>
<invoke name="prework">
<parameter name="featureName">descartes-user-manual

## Correctness Properties

*A property is a characteristic or behavior that should hold true across all valid executions of a system-essentially, a formal statement about what the system should do. Properties serve as the bridge between human-readable specifications and machine-verifiable correctness guarantees.*

Based on the prework analysis, I've identified properties that can be automatically validated and examples that require specific verification. After reviewing for redundancy, the following properties provide unique validation value:

### Property 1: Modular file structure consistency
*For any* chapter in the manual, it should have its own markdown file in the expected directory structure following the pattern `chapter_XX/README.md`
**Validates: Requirements 1.2**

### Property 2: Cross-reference link validity
*For any* cross-reference link in the manual, the target should exist and be accessible within the manual structure
**Validates: Requirements 1.4**

### Property 3: Platform installation completeness
*For any* supported platform (Linux, macOS, Windows), installation instructions should exist in the quick start guide
**Validates: Requirements 2.1**

### Property 4: Version specification consistency
*For any* Cargo.toml example in the manual, all DESCARTES dependencies should include explicit version numbers
**Validates: Requirements 2.3**

### Property 5: DES terminology definition coverage
*For any* key DES term (events, processes, simulation time, components, scheduler), it should be defined either in a glossary or inline within the conceptual foundation chapter
**Validates: Requirements 3.2**

### Property 6: Visual diagram asset existence
*For any* diagram referenced in the manual, the corresponding image file should exist in the assets directory
**Validates: Requirements 3.4**

### Property 7: Core abstraction documentation completeness
*For any* core abstraction (Simulation, Component, Scheduler, SimTime, Task), it should be documented with explanation and code examples
**Validates: Requirements 4.1, 4.2**

### Property 8: Component documentation and example coverage
*For any* available component (Server, SimpleClient, FifoQueue, PriorityQueue, retry policies), it should be documented with configuration examples
**Validates: Requirements 5.1, 5.2**

### Property 9: Custom metrics example functionality
*For any* custom metrics collection example, the code should compile and demonstrate working metrics collection
**Validates: Requirements 6.2**

### Property 10: Export format documentation coverage
*For any* supported export format (CSV, JSON, Prometheus), it should be documented with usage examples
**Validates: Requirements 6.4**

### Property 11: Visualization example functionality
*For any* visualization example, the code should compile and generate the expected output format
**Validates: Requirements 6.5**

### Property 12: Queueing example timeout and retry coverage
*For any* queueing example that includes timeout and retry policies, the code should demonstrate both timeout handling and retry mechanisms
**Validates: Requirements 7.3**

### Property 13: Tower middleware example functionality
*For any* Tower middleware example (timeout, retry, rate limiting), the code should compile and demonstrate the middleware behavior
**Validates: Requirements 8.2**

### Property 14: Multi-tier architecture example completeness
*For any* multi-tier architecture example, it should demonstrate at least two service tiers with proper communication patterns
**Validates: Requirements 8.3**

### Property 15: Advanced throttling example functionality
*For any* server-side throttling or admission control example (token buckets, RED, queue-size based), the code should compile and demonstrate the throttling behavior
**Validates: Requirements 9.3**

### Property 16: Workload generation pattern completeness
*For any* workload generation pattern (exponential arrivals, state-based clients, stateful abstractions), the code should compile and demonstrate the pattern behavior
**Validates: Requirements 9.4**

### Property 17: Custom distribution pattern example functionality
*For any* custom distribution pattern example, the code should compile and demonstrate the custom distribution behavior
**Validates: Requirements 9.5**

### Property 18: Code example compilation success
*For any* code example in the manual, it should compile without errors when extracted and built
**Validates: Requirements 10.1**

### Property 19: Example expected output inclusion
*For any* code example that produces output, the manual should include the expected output or results
**Validates: Requirements 10.2**

### Property 20: Performance benchmarking example functionality
*For any* performance benchmarking example, the code should compile and produce measurable performance metrics
**Validates: Requirements 10.5**

### Property 21: API documentation link validity
*For any* link to rustdoc API documentation, the link should be valid and point to the correct API element
**Validates: Requirements 11.1**

### Property 22: Reference table completeness
*For any* component or operation category, there should be a corresponding quick reference table with key information
**Validates: Requirements 11.2**

### Property 23: Component configuration reference coverage
*For any* component with configuration options, there should be a configuration reference section documenting all options
**Validates: Requirements 11.4**

### Property 24: Concrete example usage
*For any* concept explanation, it should include concrete code examples rather than abstract descriptions
**Validates: Requirements 12.5**

### Property 25: Asset reference completeness
*For any* referenced asset (image, CSS, JavaScript), the file should exist in the repository at the expected location
**Validates: Requirements 13.2**

## Error Handling

The manual creation and validation process includes several error handling strategies:

### Build-Time Validation

- **Link Checking**: All internal and external links are validated during build
- **Code Compilation**: All code examples are extracted and compiled as part of CI
- **Asset Verification**: All referenced assets are checked for existence
- **Markdown Syntax**: Standard markdown linting and validation

### Content Quality Assurance

- **Example Testing**: Code examples are run in isolated environments
- **Output Verification**: Expected outputs are compared against actual results
- **Cross-Reference Validation**: Internal links are verified for accuracy
- **Dependency Version Checking**: Cargo.toml examples are validated for current versions

### User Experience Safeguards

- **Progressive Complexity**: Examples build on previous knowledge
- **Error Recovery**: Common issues include troubleshooting sections
- **Alternative Approaches**: Multiple ways to accomplish tasks where appropriate
- **Clear Prerequisites**: Each section states required background knowledge

## Testing Strategy

The user manual employs a dual testing approach combining automated validation with manual review:

### Property-Based Testing

Property-based tests validate universal properties across the entire manual structure:

- **Structural Properties**: File organization, link validity, asset existence
- **Content Properties**: Example compilation, output verification, coverage completeness
- **Quality Properties**: Concrete examples, reference completeness, documentation coverage

Each property test runs with a minimum of 100 iterations to ensure comprehensive coverage. Tests are tagged with the format: **Feature: descartes-user-manual, Property {number}: {property_text}**

### Unit Testing

Unit tests focus on specific examples and validation scenarios:

- **Code Example Compilation**: Each code block is extracted and compiled
- **Expected Output Verification**: Examples with output include expected results
- **Link Validation**: Internal and external links are checked for validity
- **Asset Existence**: Referenced images and files are verified
- **Configuration Validation**: mdbook configuration is tested for correctness

### Integration Testing

Integration tests verify the complete manual build and deployment process:

- **Full Build Process**: Complete mdbook build from source to HTML
- **Search Functionality**: Search index generation and query testing
- **Navigation Testing**: Table of contents and breadcrumb functionality
- **Cross-Platform Builds**: Verification on multiple operating systems
- **Deployment Workflows**: Both local development and CI/CD pipeline testing

### Manual Review Process

Certain aspects require human review and cannot be fully automated:

- **Writing Quality**: Concise, direct language without hype
- **Readability**: Human comprehension and cognitive load
- **Practical Utility**: Real-world applicability of examples
- **Accuracy**: Technical correctness of explanations and claims

The testing strategy ensures that the manual maintains high quality while being maintainable and automatically verifiable where possible.