# DESCARTES User Manual

Welcome to the comprehensive guide for DESCARTES, a discrete event simulation framework for Rust. This manual provides everything you need to model complex distributed systems, analyze queueing networks, and conduct performance analysis with confidence.

## What is DESCARTES?

DESCARTES is a modular simulation framework built on four core crates:

- **[des-core](chapter_02/README.md)**: Core simulation abstractions, event scheduling, and task system
- **[des-components](chapter_03/README.md)**: Pre-built components for servers, clients, queues, and retry policies  
- **[des-metrics](chapter_04/README.md)**: Comprehensive metrics collection, statistical analysis, and export capabilities
- **[des-viz](chapter_04/README.md#visualization)**: Visualization tools and report generation

## Navigation Guide

### For New Users
Start here if you're new to discrete event simulation or DESCARTES:

1. **[Quick Start Guide](chapter_01/quick_start.md)** - Get running in 5 minutes
2. **[DES Basics](chapter_01/des_basics.md)** - Learn simulation fundamentals  
3. **[Core Abstractions](chapter_02/README.md)** - Understand the framework
4. **[M/M/k Queue Examples](chapter_05/README.md)** - Practice with concrete examples

### For Experienced Users
Jump to relevant sections based on your needs:

- **System Modeling**: [Components Library](chapter_03/README.md) → [Tower Integration](chapter_06/README.md)
- **Performance Analysis**: [Metrics Collection](chapter_04/README.md) → [Advanced Examples](chapter_07/README.md)
- **Complex Scenarios**: [Advanced Patterns](chapter_07/README.md) → [API Reference](reference/api.md)

### By Use Case

| **Use Case** | **Start Here** | **Then Read** |
|--------------|----------------|---------------|
| **Web Service Modeling** | [Tower Integration](chapter_06/README.md) | [Multi-tier Architectures](chapter_06/multi_tier.md) |
| **Queueing Analysis** | [M/M/k Examples](chapter_05/README.md) | [Advanced Workloads](chapter_07/workload_generation.md) |
| **Capacity Planning** | [Metrics Overview](chapter_01/metrics_overview.md) | [Performance Analysis](chapter_04/README.md) |
| **Research & Experimentation** | [DES Basics](chapter_01/des_basics.md) | [Custom Components](chapter_03/README.md#custom-components) |

## Manual Organization

This manual follows a **progressive learning approach**:

### **Foundation** (Chapters 1-2)
- **[Chapter 1: Basics of DES Simulation](chapter_01/README.md)**
  - Quick start with working examples
  - Discrete event simulation concepts
  - Metrics collection overview

- **[Chapter 2: DES-Core](chapter_02/README.md)**
  - Core abstractions and APIs
  - Component lifecycle and event processing
  - Task system and concurrency patterns

### **Building Blocks** (Chapters 3-4)
- **[Chapter 3: DES-Components](chapter_03/README.md)**
  - Pre-built simulation components
  - Configuration and composition patterns
  - Creating custom components

- **[Chapter 4: Metrics Collection](chapter_04/README.md)**
  - Metrics architecture and collection
  - Statistical analysis and export formats
  - Visualization and reporting

### **Practical Examples** (Chapters 5-6)
- **[Chapter 5: M/M/k Queue Examples](chapter_05/README.md)**
  - Basic queueing systems implementation
  - Timeout and retry policies
  - Admission control strategies

- **[Chapter 6: Tower Integration](chapter_06/README.md)**
  - Tower service abstractions
  - Middleware examples and patterns
  - Multi-tier architecture modeling

### **Advanced Patterns** (Chapter 7)
- **[Chapter 7: Advanced Examples](chapter_07/README.md)**
  - Payload-dependent service times
  - Client and server-side admission control
  - Complex workload generation patterns
  - Custom distribution implementations

### **Reference** (Appendices)
- **[API Reference](reference/api.md)** - Complete API documentation links
- **[Configuration Guide](reference/configuration.md)** - Component configuration reference
- **[Migration Guide](reference/migration.md)** - Version upgrade instructions

## Key Features

**Why Choose DESCARTES?**

- **🚀 Performance**: Native Rust performance with zero-cost abstractions
- **🔒 Safety**: Compile-time error detection and memory safety
- **🧩 Modularity**: Compose complex systems from simple, reusable components
- **📊 Observability**: Built-in metrics collection and analysis
- **🔧 Integration**: Works seamlessly with Rust ecosystem tools
- **📚 Documentation**: Comprehensive examples and clear explanations

## Code Examples

All code examples in this manual are:
- ✅ **Tested**: Automatically compiled and validated in CI
- ✅ **Complete**: Include all necessary imports and setup
- ✅ **Realistic**: Use meaningful scenarios and variable names
- ✅ **Progressive**: Build on concepts from previous chapters

## Getting Help

- **📖 API Documentation**: [docs.rs/des-core](https://docs.rs/des-core)
- **💻 Source Code**: [GitHub Repository](https://github.com/your-org/descartes)
- **🐛 Issues & Questions**: [GitHub Issues](https://github.com/your-org/descartes/issues)
- **💬 Discussions**: [GitHub Discussions](https://github.com/your-org/descartes/discussions)

---

**Ready to start?** Jump to the [Quick Start Guide](chapter_01/quick_start.md) to get DESCARTES running in minutes, or explore the [navigation guide](#navigation-guide) above to find the path that matches your experience level.

**Need something specific?** Use the search function (press `s`) to quickly find topics, code examples, or API references throughout the manual.