---

## status: accepted

# Specification: Choose an MCP server SDK for Rust

## 0. References

- Model Context Protocol Official Website: [https://modelcontextprotocol.io/](https://modelcontextprotocol.io/)

## 1. Goals

- To identify and evaluate available SDKs for implementing an MCP (Model Control Protocol) server in Rust.
- To select the most suitable MCP server SDK that aligns with project requirements and best practices.
- To ensure the chosen SDK provides robust, performant, and maintainable foundations for future development.

## 2. User Journeys

### 2.1 Developer researching SDKs

- **Scenario:** A developer is tasked with building a standalone Rust-based MCP server.
- **Action:** The developer searches for existing Rust libraries or frameworks that provide the foundation for an MCP server implementation.
- **Outcome:** The developer finds a curated list of potential SDKs, along with their key features, pros, and cons, to aid in initial assessment.

### 2.2 Architect making a decision

- **Scenario:** An architect needs to make a strategic decision on the core technology for the MCP server component.
- **Action:** The architect reviews the evaluation of identified SDKs, focusing on non-functional requirements, long-term maintainability, and community support.
- **Outcome:** The architect can confidently select an SDK, understanding the trade-offs and justifications for the choice.

## 3. Functional Requirements

- **FR1: Core MCP Protocol Support:** The chosen SDK MUST provide primitives for handling MCP messages, including parsing, serialization, and routing.
- **FR2: Command Handling:** The SDK MUST support defining and executing commands received via the MCP.
- **FR3: Event Emission:** The SDK MUST allow for emitting events according to the MCP specification.
- **FR4: Rust Ecosystem Compatibility:** The SDK MUST be well-integrated with the broader Rust ecosystem, leveraging common patterns and libraries.

## 4. Non-Functional Requirements

- **NFR1: Performance:** The SDK SHOULD offer high performance for message processing and low latency communication, suitable for real-time interactions.
  - **Measure:** Latency (ms), Throughput (messages/sec).
- **NFR2: Reliability & Stability:** The SDK MUST be stable, well-tested, and actively maintained, with a clear release cycle.
  - **Measure:** Frequency of updates, number of open issues, test coverage.
- **NFR3: Security:** The SDK SHOULD follow security best practices, including secure handling of data and potential vulnerabilities.
  - **Measure:** Security audit reports (if available), adherence to Rust security guidelines.
- **NFR4: Ease of Use & Developer Experience:** The SDK SHOULD have clear, comprehensive documentation, intuitive APIs, and examples to facilitate quick adoption and development.
  - **Measure:** Time to first "Hello World" MCP server, clarity of API.
- **NFR5: Community & Support:** The SDK SHOULD have an active community, providing resources, forums, or direct support channels.
  - **Measure:** GitHub stars/forks, activity on forums/Discord, responsiveness of maintainers.
- **NFR6: Licensing:** The SDK's license MUST be compatible with the project's overall licensing strategy.
  - **Measure:** License type (e.g., MIT, Apache 2.0).
- **NFR7: Extensibility:** The SDK SHOULD allow for easy extension and customization to accommodate future MCP protocol enhancements or specific project needs.
  - **Measure:** Presence of extension points, modular design.

## 5. Acceptance Criteria

- **AC1:** At least two distinct Rust MCP server SDKs are identified and documented.
- **AC2:** Each identified SDK is evaluated against all defined Non-Functional Requirements (NFRs), with quantitative or qualitative measures where applicable.
- **AC3:** A clear recommendation for one primary SDK is provided, along with a detailed justification based on the evaluation.
- **AC4:** A fallback or alternative SDK is identified in case the primary recommendation proves unfeasible during implementation.

## 6. Edge Cases and Error Handling

- **EC1: No Suitable SDKs Found:** If no SDKs are found that meet the minimum functional and critical non-functional requirements, the decision will be to consider building a custom, lightweight MCP server implementation or re-evaluating the project's constraints.
- **EC2: Multiple Equally Suitable SDKs:** If multiple SDKs meet all criteria with similar scores, a tie-breaking mechanism (e.g., deeper dive into specific performance benchmarks, maintainer responsiveness, or long-term roadmap) will be employed.
- **EC3: Critical Vulnerabilities/Maintenance Issues:** If an otherwise suitable SDK is found to have critical unaddressed vulnerabilities or a lack of active maintenance, it will be disqualified, and the next best alternative will be considered.
- **EC4: Licensing Conflict:** If the most suitable SDK has an incompatible license, it will be disqualified, and the next best alternative with a compatible license will be chosen.
