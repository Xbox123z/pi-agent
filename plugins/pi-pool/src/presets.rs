//! 免费模型预设：常见提供免费额度、且兼容 OpenAI 协议的上游。
//!
//! 免费额度政策随时变化，请以各平台官方说明为准；这里只提供端点与常见免费模型名。

pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub signup: &'static str,
    pub env_key: &'static str,
    pub models: &'static [&'static str],
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "openrouter",
        name: "OpenRouter (:free models)",
        base_url: "https://openrouter.ai/api/v1",
        signup: "https://openrouter.ai/keys",
        env_key: "OPENROUTER_API_KEY",
        models: &[
            "deepseek/deepseek-chat-v3.1:free",
            "deepseek/deepseek-r1:free",
            "qwen/qwen3-coder:free",
            "meta-llama/llama-3.3-70b-instruct:free",
            "z-ai/glm-4.5-air:free",
        ],
    },
    Preset {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        signup: "https://console.groq.com/keys",
        env_key: "GROQ_API_KEY",
        models: &["llama-3.3-70b-versatile", "openai/gpt-oss-120b", "qwen/qwen3-32b"],
    },
    Preset {
        id: "gemini",
        name: "Google AI Studio (Gemini)",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        signup: "https://aistudio.google.com/apikey",
        env_key: "GEMINI_API_KEY",
        models: &["gemini-2.5-flash", "gemini-2.5-flash-lite"],
    },
    Preset {
        id: "cerebras",
        name: "Cerebras",
        base_url: "https://api.cerebras.ai/v1",
        signup: "https://cloud.cerebras.ai",
        env_key: "CEREBRAS_API_KEY",
        models: &["llama-3.3-70b", "qwen-3-235b-a22b-instruct-2507", "gpt-oss-120b"],
    },
    Preset {
        id: "github",
        name: "GitHub Models",
        base_url: "https://models.github.ai/inference",
        signup: "https://github.com/settings/tokens",
        env_key: "GITHUB_TOKEN",
        models: &["openai/gpt-4.1-mini", "openai/gpt-4o-mini", "deepseek/DeepSeek-V3-0324"],
    },
    Preset {
        id: "mistral",
        name: "Mistral (free tier)",
        base_url: "https://api.mistral.ai/v1",
        signup: "https://console.mistral.ai/api-keys",
        env_key: "MISTRAL_API_KEY",
        models: &["mistral-small-latest", "codestral-latest"],
    },
    Preset {
        id: "ollama",
        name: "Ollama (local, no key)",
        base_url: "http://127.0.0.1:11434/v1",
        signup: "https://ollama.com/download",
        env_key: "OLLAMA_HOST_KEY",
        models: &["qwen2.5-coder:7b", "llama3.2"],
    },
];

pub fn find(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}
