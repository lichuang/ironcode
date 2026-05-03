//! Context — in-memory conversation history management.

use crate::llm::types::{Message, Role, ToolCall};
use crate::utils::token_counter::estimate_llm_messages_tokens;

pub struct Context {
  messages: Vec<Message>,
}

#[allow(dead_code)]
impl Context {
  pub fn with_system_prompt(prompt: impl Into<String>) -> Self {
    Self {
      messages: vec![Message::system(prompt)],
    }
  }

  pub fn from_messages(messages: Vec<Message>) -> Self {
    Self { messages }
  }

  pub fn push_user(&mut self, content: impl Into<String>) -> &Message {
    self.messages.push(Message::user(content));
    self.messages.last().unwrap()
  }

  pub fn push_assistant<T: Into<String>>(
    &mut self,
    content: impl Into<String>,
    thinking: Option<T>,
    tool_calls: Vec<ToolCall>,
  ) -> &Message {
    let reasoning_content = thinking.and_then(|t| {
      let t = t.into();
      if t.is_empty() { None } else { Some(t) }
    });

    let mut msg = if tool_calls.is_empty() {
      Message::assistant(content)
    } else {
      Message::assistant_with_tools(content, tool_calls)
    };
    msg.reasoning_content = reasoning_content;
    self.messages.push(msg);
    self.messages.last().unwrap()
  }

  pub fn push_tool_result(&mut self, tool_call_id: impl Into<String>, content: impl Into<String>) {
    self.messages.push(Message::tool(content, tool_call_id));
  }

  pub fn clear_non_system(&mut self) {
    if matches!(self.messages.first(), Some(m) if m.role == Role::System) {
      let sys = self.messages.remove(0);
      self.messages.clear();
      self.messages.push(sys);
    } else {
      self.messages.clear();
    }
  }

  pub fn messages(&self) -> &[Message] {
    &self.messages
  }

  pub fn messages_mut(&mut self) -> &mut Vec<Message> {
    &mut self.messages
  }

  pub fn estimate_tokens(&self) -> usize {
    estimate_llm_messages_tokens(&self.messages)
  }

  pub fn len(&self) -> usize {
    self.messages.len()
  }

  #[allow(dead_code)]
  pub fn is_empty(&self) -> bool {
    self.messages.is_empty()
  }
}
