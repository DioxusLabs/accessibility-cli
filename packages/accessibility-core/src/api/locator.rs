//! Locator pattern for element queries with auto-retry.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use accesskit::Action;

use crate::accessibility::{Element, TargetedAccessibility, TreeFilter, find_matches, parse_query};

use super::config::LocatorOptions;
use super::error::{Error, Result};

/// A locator for finding and interacting with elements.
///
/// Locators are lazy - they don't query elements until an action is performed.
/// When an action is called, the locator will automatically retry until the
/// element is found or the timeout is reached.
///
/// # Example
///
/// ```ignore
/// // Create a locator
/// let button = app.locator("Button[title='Submit']");
///
/// // Perform actions (with auto-retry)
/// button.click().await?;
///
/// // Query immediately without retry
/// if button.exists().await {
///     println!("Button found!");
/// }
/// ```
pub struct Locator {
    /// The underlying accessibility adapter.
    inner: Arc<Mutex<TargetedAccessibility>>,

    /// The CSS-like selector.
    selector: String,

    /// Default timeout for operations.
    timeout: Duration,

    /// Polling interval for retries.
    poll_interval: Duration,

    /// Additional options.
    options: LocatorOptions,
}

impl Locator {
    /// Create a new locator.
    pub(crate) fn new(
        inner: Arc<Mutex<TargetedAccessibility>>,
        selector: String,
        timeout: Duration,
        poll_interval: Duration,
        options: LocatorOptions,
    ) -> Self {
        Self {
            inner,
            selector,
            timeout,
            poll_interval,
            options,
        }
    }

    /// Get the selector string.
    pub fn selector(&self) -> &str {
        &self.selector
    }

    /// Set a custom timeout for this locator.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set a custom poll interval for this locator.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Set the locator to not wait (return immediately if not found).
    pub fn no_wait(mut self) -> Self {
        self.timeout = Duration::ZERO;
        self
    }

    /// Set strict mode (error on multiple matches).
    pub fn strict(mut self) -> Self {
        self.options.strict = true;
        self
    }

    /// Take the first match instead of erroring on multiple matches.
    pub fn first(mut self) -> Self {
        self.options.strict = false;
        self
    }

    /// Check if any element matches the selector (immediate, no wait).
    pub async fn exists(&self) -> bool {
        self.count().await > 0
    }

    /// Count the number of matching elements (immediate, no wait).
    pub async fn count(&self) -> usize {
        let mut inner = self.inner.lock().await;
        inner.clear_cache();
        let tree = match inner.get_tree(&TreeFilter::default()).await {
            Ok(t) => t,
            Err(_) => return 0,
        };

        let selector = match parse_query(&self.selector) {
            Ok(s) => s,
            Err(_) => return 0,
        };

        find_matches(&selector, &tree).len()
    }

    /// Get all matching elements (immediate, no wait).
    pub async fn all(&self) -> Result<Vec<Element>> {
        let mut inner = self.inner.lock().await;
        inner.clear_cache();
        let tree = inner
            .get_tree(&TreeFilter::default())
            .await
            .map_err(Error::Other)?;

        let selector = parse_query(&self.selector).map_err(|e| Error::InvalidSelector {
            selector: self.selector.clone(),
            message: e,
        })?;

        let matches = find_matches(&selector, &tree);
        Ok(matches.into_iter().cloned().collect())
    }

    /// Get the first matching element (immediate, no wait).
    pub async fn get(&self) -> Result<Option<Element>> {
        let all = self.all().await?;
        Ok(all.into_iter().next())
    }

    /// Wait for an element to match the selector.
    ///
    /// This will poll until an element is found or the timeout is reached.
    pub async fn wait(&self) -> Result<Element> {
        self.poll_for_element().await
    }

    /// Wait until the element is visible (has bounds).
    pub async fn wait_until_visible(&self) -> Result<Element> {
        self.poll_for_condition(|elem| elem.bounds.is_some(), "element visible")
            .await
    }

    /// Wait until the element is enabled.
    pub async fn wait_until_enabled(&self) -> Result<Element> {
        self.poll_for_condition(|elem| elem.enabled, "element enabled")
            .await
    }

    /// Wait until the element's `value` field contains the specified pattern.
    ///
    /// # Example
    /// ```ignore
    /// let elem = app.locator("StaticText")
    ///     .with_timeout(Duration::from_secs(5))
    ///     .wait_for_value("8")
    ///     .await?;
    /// ```
    pub async fn wait_for_value(&self, pattern: &str) -> Result<Element> {
        let pattern = pattern.to_string();
        self.poll_for_condition(
            |elem| {
                elem.value
                    .as_ref()
                    .map(|v| v.contains(&pattern))
                    .unwrap_or(false)
            },
            &format!("value contains '{}'", pattern),
        )
        .await
    }

    /// Wait until the element's text (title or description) contains the pattern.
    pub async fn wait_for_text(&self, pattern: &str) -> Result<Element> {
        let pattern = pattern.to_string();
        self.poll_for_condition(
            |elem| {
                let in_title = elem
                    .title
                    .as_ref()
                    .map(|t| t.contains(&pattern))
                    .unwrap_or(false);
                let in_desc = elem
                    .description
                    .as_ref()
                    .map(|d| d.contains(&pattern))
                    .unwrap_or(false);
                in_title || in_desc
            },
            &format!("text contains '{}'", pattern),
        )
        .await
    }

    /// Wait until a custom condition is satisfied on the element.
    ///
    /// # Example
    /// ```ignore
    /// let elem = app.locator("TextInput")
    ///     .wait_for(|e| e.enabled && e.value.as_ref().map(|v| v.len() > 5).unwrap_or(false), "enabled with long value")
    ///     .await?;
    /// ```
    pub async fn wait_for<F>(&self, predicate: F, description: &str) -> Result<Element>
    where
        F: Fn(&Element) -> bool,
    {
        self.poll_for_condition(predicate, description).await
    }

    /// Click the element.
    ///
    /// Waits for the element to be found, then performs a click action.
    pub async fn click(&self) -> Result<Element> {
        let elem = self.poll_for_element().await?;
        {
            let mut inner = self.inner.lock().await;
            inner
                .click_resolved_element(&elem)
                .await
                .map_err(|e: anyhow::Error| Error::ActionFailed {
                    action: "click".to_string(),
                    message: e.to_string(),
                })?;
        }
        Ok(elem)
    }

    /// Focus the element.
    pub async fn focus(&self) -> Result<Element> {
        let elem = self.poll_for_element().await?;
        self.perform_action(elem.id, Action::Focus, "focus").await?;
        Ok(elem)
    }

    /// Blur (remove focus from) the element.
    pub async fn blur(&self) -> Result<Element> {
        let elem = self.poll_for_element().await?;
        self.perform_action(elem.id, Action::Blur, "blur").await?;
        Ok(elem)
    }

    /// Fill the element with text (focus + set value).
    ///
    /// This focuses the element and sets its value.
    pub async fn fill(&self, text: &str) -> Result<Element> {
        let elem = self.poll_for_element().await?;

        // Focus first
        self.perform_action(elem.id, Action::Focus, "focus").await?;

        // Small delay for focus to take effect
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Set the value
        {
            let mut inner = self.inner.lock().await;
            inner
                .set_value(elem.id, text)
                .await
                .map_err(|e: anyhow::Error| Error::ActionFailed {
                    action: "fill".to_string(),
                    message: e.to_string(),
                })?;
        }

        Ok(elem)
    }

    /// Type text into the element (focus + keystroke each character).
    ///
    /// Unlike `fill()`, this simulates typing each character individually.
    pub async fn type_text(&self, text: &str) -> Result<Element> {
        let elem = self.poll_for_element().await?;

        // Focus first
        self.perform_action(elem.id, Action::Focus, "focus").await?;

        // Small delay for focus to take effect
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Type the text
        {
            let mut inner = self.inner.lock().await;
            inner
                .type_raw(text)
                .await
                .map_err(|e: anyhow::Error| Error::ActionFailed {
                    action: "type_text".to_string(),
                    message: e.to_string(),
                })?;
        }

        Ok(elem)
    }

    /// Send a keystroke to the element.
    ///
    /// # Arguments
    /// * `key_spec` - A key specification like "enter", "cmd+c", "ctrl+shift+a"
    pub async fn keystroke(&self, key_spec: &str) -> Result<Element> {
        let elem = self.poll_for_element().await?;

        // Focus first
        self.perform_action(elem.id, Action::Focus, "focus").await?;

        // Small delay for focus to take effect
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send keystroke
        {
            let mut inner = self.inner.lock().await;
            inner
                .send_keystroke(key_spec)
                .await
                .map_err(|e: anyhow::Error| Error::ActionFailed {
                    action: "keystroke".to_string(),
                    message: e.to_string(),
                })?;
        }

        Ok(elem)
    }

    /// Get the element's current value.
    pub async fn value(&self) -> Result<Option<String>> {
        let elem = self.poll_for_element().await?;
        Ok(elem.value.clone())
    }

    /// Get the element's title.
    pub async fn title(&self) -> Result<Option<String>> {
        let elem = self.poll_for_element().await?;
        Ok(elem.title.clone())
    }

    // Internal helpers

    /// Get the effective timeout (from options or default).
    fn effective_timeout(&self) -> Duration {
        self.options.timeout.unwrap_or(self.timeout)
    }

    /// Find element immediately without polling.
    async fn find_element_now(&self) -> Result<Option<Element>> {
        let mut inner = self.inner.lock().await;
        inner.clear_cache();
        let tree = inner
            .get_tree(&TreeFilter::default())
            .await
            .map_err(Error::Other)?;

        let selector = parse_query(&self.selector).map_err(|e| Error::InvalidSelector {
            selector: self.selector.clone(),
            message: e,
        })?;

        let matches = find_matches(&selector, &tree);

        if matches.is_empty() {
            return Ok(None);
        }

        if matches.len() > 1 && self.options.strict {
            return Err(Error::MultipleMatches {
                selector: self.selector.clone(),
                count: matches.len(),
            });
        }

        Ok(Some(matches[0].clone()))
    }

    /// Poll for an element until found or timeout.
    async fn poll_for_element(&self) -> Result<Element> {
        let start = Instant::now();
        let timeout = self.effective_timeout();

        loop {
            match self.find_element_now().await {
                Ok(Some(elem)) => {
                    return Ok(elem);
                }
                Ok(None) => {}
                Err(e) if !matches!(e, Error::ElementNotFound { .. }) => {
                    return Err(e);
                }
                Err(_) => {}
            }

            // Check timeout (zero timeout means no waiting)
            if timeout == Duration::ZERO || start.elapsed() >= timeout {
                return Err(Error::ElementNotFound {
                    selector: self.selector.clone(),
                });
            }

            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Poll for an element that satisfies a condition until found or timeout.
    ///
    /// This checks ALL matched elements, returning the first one that satisfies
    /// the predicate. This is useful when you have multiple matching elements
    /// and want to wait until any of them meets the condition.
    async fn poll_for_condition<F>(&self, predicate: F, condition_desc: &str) -> Result<Element>
    where
        F: Fn(&Element) -> bool,
    {
        let start = Instant::now();
        let timeout = self.effective_timeout();

        loop {
            // Check ALL matched elements, not just the first
            match self.all().await {
                Ok(elements) => {
                    // Find the first element that satisfies the predicate
                    if let Some(elem) = elements.into_iter().find(|e| predicate(e)) {
                        return Ok(elem);
                    }
                    // Elements found but none satisfy condition, keep waiting
                }
                Err(e) if !matches!(e, Error::ElementNotFound { .. }) => return Err(e),
                Err(_) => {} // No elements found, keep waiting
            }

            if timeout == Duration::ZERO || start.elapsed() >= timeout {
                return Err(Error::ConditionNotMet {
                    selector: self.selector.clone(),
                    condition: condition_desc.to_string(),
                    timeout_ms: timeout.as_millis() as u64,
                });
            }

            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Perform an action on an element.
    async fn perform_action(
        &self,
        id: crate::accessibility::ElementKey,
        action: Action,
        action_name: &str,
    ) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner
            .perform_action(id, action)
            .await
            .map_err(|e: anyhow::Error| Error::ActionFailed {
                action: action_name.to_string(),
                message: e.to_string(),
            })
    }
}

impl Clone for Locator {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            selector: self.selector.clone(),
            timeout: self.timeout,
            poll_interval: self.poll_interval,
            options: self.options.clone(),
        }
    }
}
