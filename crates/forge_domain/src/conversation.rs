use std::collections::{HashMap, VecDeque};

use derive_more::derive::Display;
use derive_setters::Setters;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{Agent, AgentId, Context, Error, Event, Result, Workflow};

#[derive(Debug, Display, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct ConversationId(Uuid);

impl ConversationId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn into_string(&self) -> String {
        self.0.to_string()
    }

    pub fn parse(value: impl ToString) -> Result<Self> {
        Ok(Self(
            Uuid::parse_str(&value.to_string()).map_err(Error::ConversationId)?,
        ))
    }
}

#[derive(Debug, Setters, Serialize, Deserialize, Clone)]
pub struct Conversation {
    pub id: ConversationId,
    pub archived: bool,
    pub state: HashMap<AgentId, AgentState>,
    pub variables: HashMap<String, Value>,
    pub agents: Vec<Agent>,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentState {
    pub turn_count: u64,
    pub context: Option<Context>,
    /// holds the events that are waiting to be processed
    pub queue: VecDeque<Event>,
}

impl Conversation {
    pub const MAIN_AGENT_NAME: &str = "software-engineer";
    pub fn new(id: ConversationId, workflow: Workflow) -> Self {
        let mut agents = Vec::new();

        for mut agent in workflow.agents.into_iter() {
            if let Some(custom_rules) = workflow.custom_rules.clone() {
                agent.custom_rules = Some(custom_rules);
            }

            if let Some(max_walker_depth) = workflow.max_walker_depth {
                agent.max_walker_depth = Some(max_walker_depth);
            }

            if let Some(temperature) = workflow.temperature {
                agent.temperature = Some(temperature);
            }

            if let Some(model) = workflow.model.clone() {
                agent.model = Some(model);
            }

            if agent.id.as_str() == Conversation::MAIN_AGENT_NAME {
                let commands = workflow
                    .commands
                    .iter()
                    .map(|c| c.name.clone())
                    .collect::<Vec<_>>();
                if let Some(ref mut subscriptions) = agent.subscribe {
                    subscriptions.extend(commands);
                } else {
                    agent.subscribe = Some(commands);
                }
            }

            agents.push(agent);
        }

        Self {
            id,
            archived: false,
            state: Default::default(),
            variables: workflow.variables.clone(),
            agents,
            events: Default::default(),
        }
    }

    pub fn turn_count(&self, id: &AgentId) -> Option<u64> {
        self.state.get(id).map(|s| s.turn_count)
    }

    /// Returns all the agents that are subscribed to the given event.
    pub fn subscriptions(&self, event_name: &str) -> Vec<Agent> {
        self.agents
            .iter()
            .filter(|a| {
                // Filter out disabled agents
                !a.disable.unwrap_or_default()
            })
            .filter(|a| {
                self.turn_count(&a.id).unwrap_or_default() < a.max_turns.unwrap_or(u64::MAX)
            })
            .filter(|a| {
                a.subscribe
                    .as_ref()
                    .is_some_and(|subs| subs.contains(&event_name.to_string()))
            })
            .cloned()
            .collect::<Vec<_>>()
    }

    /// Returns the agent with the given id or an error if it doesn't exist
    pub fn get_agent(&self, id: &AgentId) -> Result<&Agent> {
        self.agents
            .iter()
            .find(|a| a.id == *id)
            .ok_or(Error::AgentUndefined(id.clone()))
    }

    pub fn context(&self, id: &AgentId) -> Option<&Context> {
        self.state.get(id).and_then(|s| s.context.as_ref())
    }

    pub fn rfind_event(&self, event_name: &str) -> Option<&Event> {
        self.state
            .values()
            .flat_map(|state| state.queue.iter().rev())
            .find(|event| event.name == event_name)
    }

    /// Get a variable value by its key
    ///
    /// Returns None if the variable doesn't exist
    pub fn get_variable(&self, key: &str) -> Option<&Value> {
        self.variables.get(key)
    }

    /// Set a variable with the given key and value
    ///
    /// If the key already exists, its value will be updated
    pub fn set_variable(&mut self, key: String, value: Value) -> &mut Self {
        self.variables.insert(key, value);
        self
    }

    /// Delete a variable by its key
    ///
    /// Returns true if the variable was present and removed, false otherwise
    pub fn delete_variable(&mut self, key: &str) -> bool {
        self.variables.remove(key).is_some()
    }

    /// Add an event to the queue of subscribed agents
    pub fn insert_event(&mut self, event: Event) -> &mut Self {
        let subscribed_agents = self.subscriptions(&event.name);
        self.events.push(event.clone());

        subscribed_agents.iter().for_each(|agent| {
            self.state
                .entry(agent.id.clone())
                .or_default()
                .queue
                .push_back(event.clone());
        });

        self
    }

    /// Gets the next event for a specific agent, if one is available
    ///
    /// If an event is available in the agent's queue, it is popped and
    /// returned. Additionally, if the agent's queue becomes empty, it is
    /// marked as inactive.
    ///
    /// Returns None if no events are available for this agent.
    pub fn poll_event(&mut self, agent_id: &AgentId) -> Option<Event> {
        // if event is present in queue, pop it and return.
        if let Some(agent) = self.state.get_mut(agent_id) {
            if let Some(event) = agent.queue.pop_front() {
                return Some(event);
            }
        }
        None
    }

    /// Dispatches an event to all subscribed agents and activates any inactive
    /// agents
    ///
    /// This method performs two main operations:
    /// 1. Adds the event to the queue of all agents that subscribe to this
    ///    event type
    /// 2. Activates any inactive agents (where is_active=false) that are
    ///    subscribed to the event
    ///
    /// Returns a vector of AgentIds for all agents that were inactive and are
    /// now activated
    pub fn dispatch_event(&mut self, event: Event) -> Vec<AgentId> {
        let name = event.name.as_str();
        let mut agents = self.subscriptions(name);

        let inactive_agents = agents
            .iter_mut()
            .filter_map(|agent| {
                let is_inactive = self
                    .state
                    .get(&agent.id)
                    .map(|state| state.queue.is_empty())
                    .unwrap_or(true);
                if is_inactive {
                    Some(agent.id.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        self.insert_event(event);

        inactive_agents
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use crate::{Agent, Command, ModelId, Temperature, Workflow};

    #[test]
    fn test_conversation_new_with_empty_workflow() {
        // Arrange
        let id = super::ConversationId::generate();
        let workflow = Workflow {
            agents: Vec::new(),
            variables: HashMap::new(),
            commands: Vec::new(),
            model: None,
            max_walker_depth: None,
            custom_rules: None,
            temperature: None,
        };

        // Act
        let conversation = super::Conversation::new(id.clone(), workflow);

        // Assert
        assert_eq!(conversation.id, id);
        assert!(!conversation.archived);
        assert!(conversation.state.is_empty());
        assert!(conversation.variables.is_empty());
        assert!(conversation.agents.is_empty());
        assert!(conversation.events.is_empty());
    }

    #[test]
    fn test_conversation_new_with_workflow_variables() {
        // Arrange
        let id = super::ConversationId::generate();
        let mut variables = HashMap::new();
        variables.insert("key1".to_string(), json!("value1"));
        variables.insert("key2".to_string(), json!(42));

        let workflow = Workflow {
            agents: Vec::new(),
            variables: variables.clone(),
            commands: Vec::new(),
            model: None,
            max_walker_depth: None,
            custom_rules: None,
            temperature: None,
        };

        // Act
        let conversation = super::Conversation::new(id.clone(), workflow);

        // Assert
        assert_eq!(conversation.id, id);
        assert_eq!(conversation.variables, variables);
    }

    #[test]
    fn test_conversation_new_applies_workflow_settings_to_agents() {
        // Arrange
        let id = super::ConversationId::generate();
        let agent1 = Agent::new("agent1");
        let agent2 = Agent::new("agent2");

        let workflow = Workflow {
            agents: vec![agent1, agent2],
            variables: HashMap::new(),
            commands: Vec::new(),
            model: Some(ModelId::new("test-model")),
            max_walker_depth: Some(5),
            custom_rules: Some("Be helpful".to_string()),
            temperature: Some(Temperature::new(0.7).unwrap()),
        };

        // Act
        let conversation = super::Conversation::new(id.clone(), workflow);

        // Assert
        assert_eq!(conversation.agents.len(), 2);

        // Check that workflow settings were applied to all agents
        for agent in &conversation.agents {
            assert_eq!(agent.model, Some(ModelId::new("test-model")));
            assert_eq!(agent.max_walker_depth, Some(5));
            assert_eq!(agent.custom_rules, Some("Be helpful".to_string()));
            assert_eq!(agent.temperature, Some(Temperature::new(0.7).unwrap()));
        }
    }

    #[test]
    fn test_conversation_new_preserves_agent_specific_settings() {
        // Arrange
        let id = super::ConversationId::generate();

        // Agent with specific settings
        let mut agent1 = Agent::new("agent1");
        agent1.model = Some(ModelId::new("agent1-model"));
        agent1.max_walker_depth = Some(10);
        agent1.custom_rules = Some("Agent1 specific rules".to_string());
        agent1.temperature = Some(Temperature::new(0.3).unwrap());

        // Agent without specific settings
        let agent2 = Agent::new("agent2");

        let workflow = Workflow {
            agents: vec![agent1, agent2],
            variables: HashMap::new(),
            commands: Vec::new(),
            model: Some(ModelId::new("default-model")),
            max_walker_depth: Some(5),
            custom_rules: Some("Default rules".to_string()),
            temperature: Some(Temperature::new(0.7).unwrap()),
        };

        // Act
        let conversation = super::Conversation::new(id.clone(), workflow);

        // Assert
        assert_eq!(conversation.agents.len(), 2);

        // Check that agent1's settings were overridden by workflow settings
        let agent1 = conversation
            .agents
            .iter()
            .find(|a| a.id.as_str() == "agent1")
            .unwrap();
        assert_eq!(agent1.model, Some(ModelId::new("default-model")));
        assert_eq!(agent1.max_walker_depth, Some(5));
        assert_eq!(agent1.custom_rules, Some("Default rules".to_string()));
        assert_eq!(agent1.temperature, Some(Temperature::new(0.7).unwrap()));

        // Check that agent2 got the workflow defaults
        let agent2 = conversation
            .agents
            .iter()
            .find(|a| a.id.as_str() == "agent2")
            .unwrap();
        assert_eq!(agent2.model, Some(ModelId::new("default-model")));
        assert_eq!(agent2.max_walker_depth, Some(5));
        assert_eq!(agent2.custom_rules, Some("Default rules".to_string()));
        assert_eq!(agent2.temperature, Some(Temperature::new(0.7).unwrap()));
    }

    #[test]
    fn test_conversation_new_adds_commands_to_main_agent_subscriptions() {
        // Arrange
        let id = super::ConversationId::generate();

        // Create the main software-engineer agent
        let main_agent = Agent::new(super::Conversation::MAIN_AGENT_NAME);
        // Create a regular agent
        let other_agent = Agent::new("other-agent");

        // Create some commands
        let commands = vec![
            Command {
                name: "cmd1".to_string(),
                description: "Command 1".to_string(),
                prompt: None,
            },
            Command {
                name: "cmd2".to_string(),
                description: "Command 2".to_string(),
                prompt: None,
            },
        ];

        let workflow = Workflow {
            agents: vec![main_agent, other_agent],
            variables: HashMap::new(),
            commands: commands.clone(),
            model: None,
            max_walker_depth: None,
            custom_rules: None,
            temperature: None,
        };

        // Act
        let conversation = super::Conversation::new(id.clone(), workflow);

        // Assert
        assert_eq!(conversation.agents.len(), 2);

        // Check that main agent received command subscriptions
        let main_agent = conversation
            .agents
            .iter()
            .find(|a| a.id.as_str() == super::Conversation::MAIN_AGENT_NAME)
            .unwrap();

        assert!(main_agent.subscribe.is_some());
        let subscriptions = main_agent.subscribe.as_ref().unwrap();
        assert!(subscriptions.contains(&"cmd1".to_string()));
        assert!(subscriptions.contains(&"cmd2".to_string()));

        // Check that other agent didn't receive command subscriptions
        let other_agent = conversation
            .agents
            .iter()
            .find(|a| a.id.as_str() == "other-agent")
            .unwrap();

        if other_agent.subscribe.is_some() {
            assert!(!other_agent
                .subscribe
                .as_ref()
                .unwrap()
                .contains(&"cmd1".to_string()));
            assert!(!other_agent
                .subscribe
                .as_ref()
                .unwrap()
                .contains(&"cmd2".to_string()));
        }
    }

    #[test]
    fn test_conversation_new_merges_commands_with_existing_subscriptions() {
        // Arrange
        let id = super::ConversationId::generate();

        // Create the main software-engineer agent with existing subscriptions
        let mut main_agent = Agent::new(super::Conversation::MAIN_AGENT_NAME);
        main_agent.subscribe = Some(vec!["existing-event".to_string()]);

        // Create some commands
        let commands = vec![
            Command {
                name: "cmd1".to_string(),
                description: "Command 1".to_string(),
                prompt: None,
            },
            Command {
                name: "cmd2".to_string(),
                description: "Command 2".to_string(),
                prompt: None,
            },
        ];

        let workflow = Workflow {
            agents: vec![main_agent],
            variables: HashMap::new(),
            commands: commands.clone(),
            model: None,
            max_walker_depth: None,
            custom_rules: None,
            temperature: None,
        };

        // Act
        let conversation = super::Conversation::new(id.clone(), workflow);

        // Assert
        let main_agent = conversation
            .agents
            .iter()
            .find(|a| a.id.as_str() == super::Conversation::MAIN_AGENT_NAME)
            .unwrap();

        assert!(main_agent.subscribe.is_some());
        let subscriptions = main_agent.subscribe.as_ref().unwrap();

        // Should contain both the existing subscription and the new commands
        assert!(subscriptions.contains(&"existing-event".to_string()));
        assert!(subscriptions.contains(&"cmd1".to_string()));
        assert!(subscriptions.contains(&"cmd2".to_string()));
        assert_eq!(subscriptions.len(), 3);
    }
}
