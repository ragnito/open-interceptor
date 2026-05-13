// Anthropic Messages API ↔ OpenAI Chat Completions translation layer.
//
// Phase 3. Includes both directions and (eventually) streaming SSE
// re-encoding so chunks flow client-side without full buffering.

pub mod types_anthropic;
pub mod types_openai;
