//! Dispatch settings draft and presentation rules, shared by GPUI and fixture replay.

use super::{
    AGENT_IDS, CONFIG_RELATIVE_PATH, DispatchConfigSnapshot, DispatchDefaults, EFFORT_IDS,
    js_whitespace,
};

pub const SCOPE_NOTE: &str = "只影响新派的任务；单个任务仍可用命令行参数覆盖。";
pub const MISSING_NOTE: &str = "文件尚未创建，首次保存时会自动创建。";
pub const SAVED_MESSAGE: &str = "已保存，新派任务生效";
pub const LOAD_FAILED: &str = "无法读取派活默认值，请重试。";
pub const RETRY_LABEL: &str = "重试";
const SAVE_FAILED: &str = "保存失败，修改已保留。";
const CODEX_MODELS: [&str; 5] = ["sol", "best", "astra", "terra", "luna"];
const ORCHESTRATOR: &str = "gemini-pro-orchestrator";
const AGY_MODELS: [&str; 4] = [ORCHESTRATOR, "hydrogen", "argon", "carbon"];
const DSH_MODEL: &str = "deepseek-flash";
const DSH_EFFORTS: [&str; 2] = ["low", "high"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceField {
    Agent,
    Model,
    Effort,
}

impl ChoiceField {
    pub const ALL: [Self; 3] = [Self::Agent, Self::Model, Self::Effort];

    pub fn key(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Model => "model",
            Self::Effort => "effort",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "默认执行者",
            Self::Model => "默认模型",
            Self::Effort => "推理档位",
        }
    }
}

pub struct ChoiceOption {
    pub value: &'static str,
    pub label: &'static str,
}

pub struct ChoiceView {
    pub value: String,
    pub shown_label: String,
    pub options: Vec<ChoiceOption>,
    pub disabled: bool,
    pub read_only: bool,
    pub hints: Vec<String>,
}

impl ChoiceView {
    fn new(value: &str, options: &[&'static str], label: impl Fn(&str) -> &str) -> Self {
        Self {
            value: value.into(),
            shown_label: label(value).into(),
            options: options
                .iter()
                .map(|&value| ChoiceOption {
                    value,
                    label: label(value),
                })
                .collect(),
            disabled: false,
            read_only: false,
            hints: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberField {
    IdleMin,
    MaxMin,
    AskWaitMin,
    Retries,
}

impl NumberField {
    pub const ALL: [Self; 4] = [Self::IdleMin, Self::MaxMin, Self::AskWaitMin, Self::Retries];

    pub fn key(self) -> &'static str {
        match self {
            Self::IdleMin => "idleMin",
            Self::MaxMin => "maxMin",
            Self::AskWaitMin => "askWaitMin",
            Self::Retries => "retries",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::IdleMin => "没动静判卡",
            Self::MaxMin => "单轮最长",
            Self::AskWaitMin => "等回答",
            Self::Retries => "重试次数",
        }
    }

    pub fn unit(self) -> &'static str {
        if self == Self::Retries { "" } else { "分钟" }
    }

    fn limits(self) -> (u16, u16) {
        match self {
            Self::IdleMin | Self::AskWaitMin => (1, 60),
            Self::MaxMin => (5, 180),
            Self::Retries => (0, 5),
        }
    }

    fn get(self, defaults: &DispatchDefaults) -> f64 {
        match self {
            Self::IdleMin => defaults.idle_min,
            Self::MaxMin => defaults.max_min,
            Self::AskWaitMin => defaults.ask_wait_min,
            Self::Retries => f64::from(defaults.retries),
        }
    }

    fn set(self, defaults: &mut DispatchDefaults, value: f64) {
        match self {
            Self::IdleMin => defaults.idle_min = value,
            Self::MaxMin => defaults.max_min = value,
            Self::AskWaitMin => defaults.ask_wait_min = value,
            Self::Retries => defaults.retries = value as u8,
        }
    }

    fn parse(self, text: &str, saved: f64) -> Result<f64, String> {
        let raw = text.trim_matches(js_whitespace);
        let integer = self == Self::Retries;
        let number_type = if integer { "整数" } else { "数字" };
        // Previously saved values keep their validity even outside editor limits.
        if raw == number_text(saved) {
            return Ok(saved);
        }
        let mut parts = raw.split('.');
        let all_digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        if !parts.next().is_some_and(all_digits)
            || !parts.next().is_none_or(all_digits)
            || parts.next().is_some()
        {
            return Err(format!("请输入{number_type}。"));
        }
        let value: f64 = raw.parse().unwrap_or(f64::INFINITY);
        if value == saved {
            return Ok(saved);
        }
        if integer && (!value.is_finite() || value.fract() != 0.0) {
            return Err(format!("请输入{number_type}。"));
        }
        let (min, max) = self.limits();
        if !(f64::from(min)..=f64::from(max)).contains(&value) {
            return Err(format!("请输入 {min}–{max} 之间的{number_type}。"));
        }
        Ok(value)
    }
}

fn number_text(value: f64) -> String {
    if value > 0.0 && value < 1e-6 {
        format!("{value:e}")
    } else {
        value.to_string()
    }
}

pub struct DispatchForm {
    snapshot: DispatchConfigSnapshot,
    draft: DispatchDefaults,
    numbers: [String; 4],
    errors: [Option<String>; 4],
    saved: bool,
    save_error: Option<String>,
}

impl DispatchForm {
    pub fn new(snapshot: DispatchConfigSnapshot) -> Result<Self, &'static str> {
        let draft = snapshot.defaults.clone();
        if !AGENT_IDS.contains(&draft.agent.as_str())
            || !EFFORT_IDS.contains(&draft.effort.as_str())
            || draft.model.is_empty()
            || !NumberField::ALL
                .iter()
                .all(|field| field.get(&draft).is_finite())
        {
            return Err(LOAD_FAILED);
        }
        Ok(Self {
            numbers: NumberField::ALL.map(|field| number_text(field.get(&draft))),
            snapshot,
            draft,
            errors: Default::default(),
            saved: false,
            save_error: None,
        })
    }

    pub fn exists(&self) -> bool {
        self.snapshot.exists
    }

    pub fn config_path(&self) -> String {
        let path = self.snapshot.path.to_string_lossy();
        if path.replace('\\', "/").ends_with(CONFIG_RELATIVE_PATH) || path.is_empty() {
            format!("~/{CONFIG_RELATIVE_PATH}")
        } else {
            path.into_owned()
        }
    }

    pub fn choice(&self, field: ChoiceField) -> ChoiceView {
        let agent = self.draft.agent.as_str();
        let model = self.draft.model.as_str();
        let effort = self.draft.effort.as_str();
        let models = if agent == "agy" {
            &AGY_MODELS[..]
        } else {
            &CODEX_MODELS[..]
        };
        let model_is_listed = models.contains(&model);
        let shown_model = if model_is_listed { model } else { models[0] };
        match field {
            ChoiceField::Agent => ChoiceView::new(agent, &AGENT_IDS, |agent| match agent {
                "agy" => "Antigravity",
                "dsh" => "DSH",
                _ => "Codex",
            }),
            ChoiceField::Model if agent == "dsh" => {
                let mut view = ChoiceView::new(DSH_MODEL, &[], |model| model);
                view.read_only = true;
                view.hints
                    .push("DSH 型号固定，配置文件里的 model 对它不生效".into());
                view
            }
            ChoiceField::Model => {
                let mut view = ChoiceView::new(shown_model, models, |model| {
                    if model == ORCHESTRATOR {
                        "Orchestrator（固定）"
                    } else {
                        model
                    }
                });
                if !model_is_listed {
                    view.hints.push("当前模型不在该执行者的预设列表中，这里显示默认项。直接保存会保留原模型值。".into());
                }
                view
            }
            ChoiceField::Effort if agent == "dsh" => {
                let clamped = !DSH_EFFORTS.contains(&effort);
                let mut view = ChoiceView::new(
                    if clamped { "high" } else { effort },
                    &DSH_EFFORTS,
                    effort_label,
                );
                view.hints.push("DSH 只支持低/高".into());
                if clamped {
                    view.hints.push(format!(
                        "当前档位为「{}」，DSH 按「高」显示。直接保存会保留原档位。",
                        effort_label(effort)
                    ));
                }
                view
            }
            ChoiceField::Effort => {
                let mut view = ChoiceView::new(effort, &EFFORT_IDS, effort_label);
                if agent == "agy" && shown_model == ORCHESTRATOR {
                    view.disabled = true;
                    view.hints
                        .push("Orchestrator 没有独立的推理档位，沿用它的内置档位。".into());
                }
                view
            }
        }
    }

    pub fn choose(&mut self, field: ChoiceField, value: &str) {
        let view = self.choice(field);
        if view.disabled
            || view.read_only
            || !view.options.iter().any(|option| option.value == value)
        {
            return;
        }
        match field {
            ChoiceField::Agent if value == self.draft.agent => return,
            ChoiceField::Agent => {
                self.draft.agent = value.into();
                self.draft.model.clone_from(&self.snapshot.defaults.model);
            }
            ChoiceField::Model => self.draft.model = value.into(),
            ChoiceField::Effort => self.draft.effort = value.into(),
        }
        self.mark_edited();
    }

    pub fn number(&self, field: NumberField) -> &str {
        &self.numbers[field as usize]
    }

    pub fn number_error(&self, field: NumberField) -> Option<&str> {
        self.errors[field as usize].as_deref()
    }

    pub fn edit_number(&mut self, field: NumberField, text: String) {
        self.numbers[field as usize] = text;
        self.errors[field as usize] = None;
        self.mark_edited();
    }

    pub fn commit_number(&mut self, field: NumberField) {
        self.errors[field as usize] = self.parse_number(field).err();
    }

    pub fn restore_defaults(&mut self) {
        self.draft = DispatchDefaults::default();
        self.numbers = NumberField::ALL.map(|field| number_text(field.get(&self.draft)));
        self.errors = Default::default();
        self.mark_edited();
    }

    pub fn is_dirty(&self) -> bool {
        self.draft.agent != self.snapshot.defaults.agent
            || self.draft.model != self.snapshot.defaults.model
            || self.draft.effort != self.snapshot.defaults.effort
            || NumberField::ALL
                .into_iter()
                .any(|field| self.parse_number(field) != Ok(field.get(&self.snapshot.defaults)))
    }

    pub fn can_save(&self) -> bool {
        self.is_dirty()
            && NumberField::ALL
                .into_iter()
                .all(|field| self.parse_number(field).is_ok())
    }

    pub fn saved(&self) -> bool {
        self.saved
    }

    pub fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref()
    }

    /// Revalidate the draft and return the first invalid field for the view to focus.
    pub fn save(
        &mut self,
        write: impl FnOnce(DispatchDefaults) -> Result<DispatchConfigSnapshot, String>,
    ) -> Option<NumberField> {
        if !self.is_dirty() {
            return None;
        }
        let mut next = self.draft.clone();
        for field in NumberField::ALL {
            self.errors[field as usize] = match self.parse_number(field) {
                Ok(value) => {
                    field.set(&mut next, value);
                    None
                }
                Err(error) => Some(error),
            };
        }
        let first_error = NumberField::ALL
            .into_iter()
            .find(|&field| self.number_error(field).is_some());
        if first_error.is_some() {
            return first_error;
        }
        self.mark_edited();
        let result = write(next).and_then(|snapshot| {
            Self::new(snapshot).map_err(|_| "服务器返回的配置无法识别".to_owned())
        });
        match result {
            Ok(mut saved) => {
                saved.saved = true;
                *self = saved;
            }
            Err(error) => self.save_error = Some(format!("{SAVE_FAILED}{error}")),
        }
        None
    }

    fn parse_number(&self, field: NumberField) -> Result<f64, String> {
        field.parse(self.number(field), field.get(&self.snapshot.defaults))
    }

    fn mark_edited(&mut self) {
        self.saved = false;
        self.save_error = None;
    }
}

fn effort_label(effort: &str) -> &str {
    match effort {
        "low" => "低",
        "medium" => "中",
        "high" => "高",
        _ => "超高",
    }
}
