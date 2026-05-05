//! Element cache for storing platform handles with slotmap-based IDs.

use super::types::{Element, ElementKey};
use slotmap::SlotMap;

/// Cache for accessibility elements with slotmap-based ID assignment.
///
/// This cache stores elements and assigns them IDs using slotmap, which provides:
/// - Automatic generation counters for stale-key detection
/// - Efficient O(1) insertion, lookup, and removal
/// - Automatic slot reuse after removal
///
/// The cache is invalidated when `clear()` is called, incrementing the snapshot version.
/// After clear, any previously-issued ElementKeys will automatically fail lookups due to
/// slotmap's generation counter mechanism.
///
/// Platform-specific implementations store their native handles in `SecondaryMap<ElementKey, T>`
/// alongside this cache.
#[derive(Debug)]
pub struct ElementCache {
    /// Cached elements indexed by their slotmap key.
    elements: SlotMap<ElementKey, Element>,

    /// Snapshot version (increments on clear).
    version: u64,
}

impl Default for ElementCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ElementCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self {
            elements: SlotMap::with_key(),
            version: 1,
        }
    }

    /// Clear the cache and increment the snapshot version.
    ///
    /// Call this before taking a new snapshot to invalidate old IDs.
    /// After clearing, any previously-issued ElementKeys will return `None`
    /// when used with `get()` due to slotmap's generation counters.
    pub fn clear(&mut self) {
        self.elements.clear();
        self.version += 1;
    }

    /// Store an element and return its assigned ID.
    pub fn store(&mut self, mut element: Element) -> ElementKey {
        self.elements.insert_with_key(|key| {
            element.id = key;
            element
        })
    }

    /// Store an element using a closure that receives the assigned ID.
    ///
    /// Returns only the ID. Use `store_with_clone()` if you also need the element.
    ///
    /// # Example
    /// ```ignore
    /// let id = cache.store_with(|id| {
    ///     let mut elem = Element::new(id, Role::Button);
    ///     elem.title = Some("Click Me".to_string());
    ///     elem
    /// });
    /// ```
    pub fn store_with<F>(&mut self, f: F) -> ElementKey
    where
        F: FnOnce(ElementKey) -> Element,
    {
        self.elements.insert_with_key(f)
    }

    /// Store an element using a closure and return both the ID and a clone of the element.
    ///
    /// This is useful when you need to both store an element and return it (e.g., when
    /// building a tree structure where the returned element is added to a parent's children).
    ///
    /// # Example
    /// ```ignore
    /// let (id, element) = cache.store_with_clone(|id| {
    ///     let mut elem = Element::new(id, Role::Button);
    ///     elem.title = Some("Click Me".to_string());
    ///     elem
    /// });
    /// // element.id == id, and element is stored in cache
    /// ```
    pub fn store_with_clone<F>(&mut self, f: F) -> (ElementKey, Element)
    where
        F: FnOnce(ElementKey) -> Element,
    {
        let key = self.elements.insert_with_key(f);
        let element = self.elements[key].clone();
        (key, element)
    }

    /// Get an element by its ID.
    pub fn get(&self, id: ElementKey) -> Option<&Element> {
        self.elements.get(id)
    }

    /// Get a mutable reference to an element by its ID.
    pub fn get_mut(&mut self, id: ElementKey) -> Option<&mut Element> {
        self.elements.get_mut(id)
    }

    /// Check if an ID exists in the cache.
    pub fn contains(&self, id: ElementKey) -> bool {
        self.elements.contains_key(id)
    }

    /// Get the current snapshot version.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Get the number of cached elements.
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// Iterate over all cached elements.
    ///
    /// Returns an iterator of `(ElementKey, &Element)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (ElementKey, &Element)> {
        self.elements.iter()
    }

    /// Get all element IDs in the cache.
    pub fn ids(&self) -> impl Iterator<Item = ElementKey> + '_ {
        self.elements.keys()
    }

    /// Reserve an ID without storing an element yet.
    ///
    /// # Deprecated
    /// Use `store()` or `store_with()` instead. Reserving IDs creates placeholder
    /// elements that waste memory.
    #[deprecated(since = "0.2.0", note = "Use store() or store_with() instead")]
    pub fn reserve_id(&mut self) -> ElementKey {
        self.elements
            .insert_with_key(|key| Element::new(key, accesskit::Role::Unknown))
    }

    /// Allocate the next ID without storing an element.
    ///
    /// # Deprecated
    /// Use `store()` or `store_with()` instead.
    #[deprecated(since = "0.2.0", note = "Use store() or store_with() instead")]
    pub fn next_id(&mut self) -> ElementKey {
        #[allow(deprecated)]
        self.reserve_id()
    }

    /// Store an element with a pre-allocated ID.
    ///
    /// # Deprecated
    /// Use `store()` or `store_with()` instead. This method is kept for backwards
    /// compatibility with code that uses `reserve_id()`.
    #[deprecated(since = "0.2.0", note = "Use store() or store_with() instead")]
    pub fn store_with_id(&mut self, id: ElementKey, mut element: Element) {
        if self.elements.contains_key(id) {
            // Update existing slot (reserved ID)
            element.id = id;
            self.elements[id] = element;
            return;
        }
        // ID doesn't exist - insert as new element
        let key = self.elements.insert_with_key(|key| {
            element.id = key;
            element
        });
        let _ = key;
    }

    /// Get the underlying slotmap for direct key access.
    ///
    /// This is useful for platform implementations that need to work with
    /// `SecondaryMap<ElementKey, T>` directly.
    pub fn slotmap(&self) -> &SlotMap<ElementKey, Element> {
        &self.elements
    }

    /// Get a mutable reference to the underlying slotmap.
    pub fn slotmap_mut(&mut self) -> &mut SlotMap<ElementKey, Element> {
        &mut self.elements
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use accesskit::Role;

    #[test]
    fn test_store_and_get() {
        let mut cache = ElementCache::new();

        let id1 = cache.store_with(|id| Element::new(id, Role::Button));
        let id2 = cache.store_with(|id| Element::new(id, Role::TextInput));
        let id3 = cache.store_with(|id| Element::new(id, Role::CheckBox));

        // IDs should be unique (but not necessarily sequential due to slotmap)
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);

        // All should be retrievable
        assert!(cache.get(id1).is_some());
        assert!(cache.get(id2).is_some());
        assert!(cache.get(id3).is_some());
    }

    #[test]
    fn test_clear_invalidates_old_ids() {
        let mut cache = ElementCache::new();

        let id = cache.store_with(|id| Element::new(id, Role::Button));
        assert!(cache.get(id).is_some());

        let v1 = cache.version();
        cache.clear();
        let v2 = cache.version();

        assert_eq!(v2, v1 + 1);

        // Old ID should no longer work due to generation counter
        assert!(cache.get(id).is_none());

        // New ID should work
        let new_id = cache.store_with(|id| Element::new(id, Role::Button));
        assert!(cache.get(new_id).is_some());
    }

    #[test]
    fn test_get_element() {
        let mut cache = ElementCache::new();

        let id = cache.store_with(|id| {
            let mut elem = Element::new(id, Role::Button);
            elem.title = Some("Click Me".to_string());
            elem
        });
        let retrieved = cache.get(id).unwrap();

        assert_eq!(retrieved.title.as_deref(), Some("Click Me"));
        assert_eq!(retrieved.role, Role::Button);
    }

    #[test]
    #[allow(deprecated)]
    fn test_reserve_and_store_with_id() {
        let mut cache = ElementCache::new();

        // Reserve an ID
        let id = cache.next_id();

        // The placeholder should exist
        assert!(cache.get(id).is_some());

        // Now store the actual element
        let mut elem = Element::new(id, Role::Button);
        elem.title = Some("Reserved Button".to_string());
        cache.store_with_id(id, elem);

        // Should still be retrievable with the same ID
        let retrieved = cache.get(id).unwrap();
        assert_eq!(retrieved.title.as_deref(), Some("Reserved Button"));
        assert_eq!(retrieved.role, Role::Button);
    }

    #[test]
    fn test_store_with() {
        let mut cache = ElementCache::new();

        let id = cache.store_with(|id| {
            let mut elem = Element::new(id, Role::Button);
            elem.title = Some("Built with ID".to_string());
            elem
        });

        let retrieved = cache.get(id).unwrap();
        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.title.as_deref(), Some("Built with ID"));
        assert_eq!(retrieved.role, Role::Button);
    }

    #[test]
    fn test_ffi_roundtrip() {
        let mut cache = ElementCache::new();
        let id = cache.store_with(|id| Element::new(id, Role::Button));

        // Convert to FFI and back
        let ffi_value = id.to_ffi();
        let recovered = ElementKey::from_ffi(ffi_value);

        assert_eq!(id, recovered);
        assert!(cache.get(recovered).is_some());
    }
}
