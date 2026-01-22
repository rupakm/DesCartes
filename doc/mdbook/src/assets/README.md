# DESCARTES User Manual Assets

This directory contains templates and assets for maintaining consistency across the DESCARTES user manual.

## Templates

### Chapter Template (`chapter_template.md`)
Use this template when creating new chapters. It provides:

- Consistent structure and formatting
- Standard sections for learning objectives, prerequisites, and examples
- Navigation and cross-reference patterns
- Troubleshooting and best practices sections

**Usage:**
1. Copy `chapter_template.md` to the appropriate chapter directory
2. Rename to `README.md` 
3. Fill in the template sections with chapter-specific content
4. Update cross-references and navigation links

### Section Template (`section_template.md`)
Use this template for individual sections within chapters. It provides:

- Step-by-step implementation structure
- Code example formatting standards
- Configuration and error handling patterns
- Testing and performance guidance

**Usage:**
1. Copy `section_template.md` to the chapter directory
2. Rename to match the section topic (e.g., `quick_start.md`)
3. Fill in the template with section-specific content
4. Ensure code examples compile and include expected output

## Content Standards

### Code Examples
All code examples must:
- ✅ **Compile**: Include all necessary imports and dependencies
- ✅ **Run**: Provide complete, executable examples
- ✅ **Realistic**: Use meaningful variable names and scenarios
- ✅ **Commented**: Explain non-obvious parts and key concepts
- ✅ **Progressive**: Build on concepts from previous sections

### Cross-References
Use consistent linking patterns:
- **Internal links**: `[Text](../chapter_XX/section.md)`
- **API references**: `[Function](https://docs.rs/des-core/latest/des_core/fn.function.html)`
- **Anchors**: `[Text](#section-heading)`

### Formatting Standards
- **Headings**: Use sentence case (not title case)
- **Code**: Use `code` for inline code, triple backticks for blocks
- **Emphasis**: Use **bold** for important terms, *italic* for emphasis
- **Lists**: Use bullet points for unordered, numbers for sequential steps

## Asset Organization

```
assets/
├── README.md              # This file
├── chapter_template.md    # Template for new chapters
├── section_template.md    # Template for new sections
├── images/               # Diagrams and screenshots
├── code/                 # Complete code examples
└── css/                  # Additional styling (if needed)
```

## Quality Checklist

Before publishing new content, verify:

- [ ] All code examples compile and run
- [ ] Cross-references point to existing content
- [ ] Images and assets are properly referenced
- [ ] Content follows the template structure
- [ ] Examples include expected output
- [ ] Prerequisites are clearly stated
- [ ] Navigation links are updated

## Maintenance

When updating templates:
1. Update this README with any changes
2. Consider impact on existing chapters
3. Update the custom CSS if styling changes are needed
4. Test template changes with a sample chapter

## Custom CSS

The manual uses custom CSS (`../theme/custom.css`) for:
- Consistent code block styling
- Table formatting and responsive design
- Navigation and search improvements
- Dark theme support
- Print-friendly styles

Refer to the CSS file for specific styling classes and patterns.