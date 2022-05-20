use crate::rcm::ResourceError::NotEnough;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use tracing::warn;

/// Resource Manager
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ResourceError {
    NotRegistered,
    Unknown,
    /// Not enough resources in the pool. Requested, Current Amount
    NotEnough(usize, usize),
}

#[derive(Debug, Clone)]
pub struct Resource {
    resource: String,
    lake: Arc<Mutex<usize>>,
}
impl Resource {
    pub fn consume(&self, amount: usize) -> Result<Claim, ResourceError> {
        let mut handle = self.lake.lock().unwrap();
        if amount > *handle {
            warn!(
                "attempted to consume {} units of {}, but only {} units exist",
                &amount, &self.resource, &*handle
            );
            return Err(NotEnough(amount, *handle));
        }

        // reduce the amount stored by the amount we want
        *handle -= amount;

        Ok(Claim {
            amount,
            lake: self.lake.clone(),
        })
    }
    pub fn peak(&self) -> usize {
        *self.lake.lock().unwrap()
    }
}
/// A claim represent a chunk of the ResourcePool that is being held
#[derive(Debug)]
pub struct Claim {
    amount: usize,
    lake: Arc<Mutex<usize>>,
}
impl Claim {
    /// Release this claim, returning the resources to the pool at large. This action consumes the
    /// object
    pub fn release(self) {
        self.int_release()
    }
    pub fn read(&self) -> usize {
        self.amount
    }
    fn int_release(&self) {
        let mut h = self.lake.lock().unwrap();
        *h += self.amount;
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        self.int_release()
    }
}

#[derive(Debug, Clone)]
pub struct ResourceManager {
    /// Map of Resources
    resource_lake: Arc<Mutex<HashMap<String, Resource>>>,
}
impl ResourceManager {
    pub fn new() -> Self {
        ResourceManager {
            resource_lake: Arc::new(Default::default()),
        }
    }

    /// Add a resource amount to the pool. Incrementing it by the given amount if it does not exist
    pub fn add(&self, resource: impl Into<String>, amount: usize) {
        let resource = resource.into();
        let mut h = self.resource_lake.lock().unwrap();
        if let Some(r) = h.get(resource.as_str()) {
            let mut resource_lake = r.lake.lock().unwrap();
            *resource_lake += amount;
        } else {
            h.insert(
                resource.clone(),
                Resource {
                    resource: resource.clone(),
                    lake: Arc::new(Mutex::new(amount)),
                },
            );
        }
    }

    /// Remove the given amount of resources from the given resource. If the resulting amount of
    /// resource is less than or equal to zero, the Resource is removed.
    pub fn remove(
        &self,
        _resource: impl Into<String>,
        _amount: usize,
    ) -> Result<(), ResourceError> {
        // let mut h = self.resource_lake.lock().unwrap();
        // h.remove(resource.into().as_str());
        // Ok(())
        todo!()
    }

    /// Consume a given amount of the given resource. Returns a `Claim` object, which represents the
    /// block of resources. Once the `Claim` object goes out of scope/is dropped, the resources are
    /// returned to the pool
    pub fn consume(
        &self,
        resource: impl Into<String>,
        amount: usize,
    ) -> Result<Claim, ResourceError> {
        let resource = resource.into();
        let h = self.resource_lake.lock().unwrap();
        if let Some(r) = h.get(resource.as_str()) {
            r.consume(amount)
        } else {
            warn!(resource = resource.as_str(), "resource does not exist");
            Err(ResourceError::Unknown)
        }
    }

    pub fn peak(&self) -> HashMap<String, usize> {
        let mut output = HashMap::new();
        for (k, v) in self.resource_lake.lock().unwrap().iter() {
            output.insert(k.clone(), v.peak());
        }

        output
    }
}
impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::rcm::{ResourceError, ResourceManager};

    #[test]
    fn test_resource_manager() {
        let rcm = ResourceManager::new();
        rcm.add("test1", 1024);
        rcm.add("test2", 512);

        let current = rcm.peak();

        assert_eq!(current.get("test1"), Some(&1024));
        assert_eq!(current.get("test2"), Some(&512));

        // try an invalid claim
        let chunk = rcm.consume("test2", 1024);
        assert_eq!(chunk.is_err(), true);
        assert_eq!(chunk.err().unwrap(), ResourceError::NotEnough(1024, 512));

        let chunk = rcm.consume("test1", 512);
        assert_eq!(chunk.is_ok(), true);
        let claim = chunk.unwrap();
        assert_eq!(claim.read(), 512);

        // peak at the current values
        let current = rcm.peak();

        assert_eq!(current.get("test1"), Some(&512));

        // drop the claim- which should return the resources back to the main pool
        drop(claim);

        let current = rcm.peak();

        assert_eq!(current.get("test1"), Some(&1024));
        assert_eq!(current.get("test2"), Some(&512));
    }
}
