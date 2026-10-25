use super::*;
use std::cell::{Cell, RefCell};

struct Mock {
    item: RefCell<Option<Vec<u8>>>,
    account: String,
    replace_on_delete: bool,
    fail_after_delete: Cell<bool>,
    deletes: Cell<usize>,
}
impl Items for Mock {
    fn exists(&self, reference: &[u8]) -> Result<bool> {
        Ok(self.item.borrow().as_deref() == Some(reference))
    }
    fn lookup(&self, _: &str, account: &str) -> Result<Option<Vec<u8>>> {
        Ok((account == self.account)
            .then(|| self.item.borrow().clone())
            .flatten())
    }
    fn delete(&self, _: &str, account: &str, reference: &[u8]) -> Result<()> {
        assert_eq!(account, self.account);
        self.deletes.set(self.deletes.get() + 1);
        if self.replace_on_delete {
            *self.item.borrow_mut() = Some(vec![2]);
        }
        if self.item.borrow().as_deref() == Some(reference) {
            self.item.borrow_mut().take();
        }
        if self.fail_after_delete.replace(false) {
            return Err(Error::state_io("Injected post-deletion crash."));
        }
        Ok(())
    }
}
fn fixture() -> (tempfile::TempDir, Generation, Mock) {
    let temp = tempfile::tempdir().unwrap();
    let state = state::State::new(temp.path().join("state")).unwrap();
    let profile = crate::config::Profile {
        provider: crate::config::Provider::Claude,
        method: crate::config::Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.register("claude", &profile).unwrap();
    let generation = state
        .read_index()
        .unwrap()
        .into_generation(binding.id)
        .unwrap();
    let mock = Mock {
        item: RefCell::new(Some(vec![1])),
        account: generation.keychain_account.clone().unwrap(),
        replace_on_delete: false,
        fail_after_delete: Cell::new(false),
        deletes: Cell::new(0),
    };
    (temp, generation, mock)
}
#[test]
fn missing_or_mismatched_account_refuses_without_deletion_or_intent() {
    for mismatch in [false, true] {
        let (_temp, generation, mut mock) = fixture();
        if mismatch {
            mock.account = "another-account".into();
        } else {
            mock.item.borrow_mut().take();
        }
        assert!(remove_with(&generation, &mock).is_err());
        assert_eq!(mock.deletes.get(), 0);
        assert!(
            !generation
                .binding
                .directory
                .join("native-removal.json")
                .exists()
        );
    }
}
#[test]
fn durable_identity_allows_retry_after_deletion_but_never_a_replacement() {
    let (_temp, generation, mock) = fixture();
    mock.fail_after_delete.set(true);
    assert!(remove_with(&generation, &mock).is_err());
    assert!(mock.item.borrow().is_none());
    remove_with(&generation, &mock).unwrap();
    assert_eq!(mock.deletes.get(), 1);
    *mock.item.borrow_mut() = Some(vec![2]);
    assert!(remove_with(&generation, &mock).is_err());
    assert_eq!(mock.deletes.get(), 1);
    assert_eq!(*mock.item.borrow(), Some(vec![2]));
}
#[test]
fn replacement_between_lookup_and_delete_is_not_deleted() {
    let (_temp, generation, mut mock) = fixture();
    mock.replace_on_delete = true;
    assert!(remove_with(&generation, &mock).is_err());
    assert_eq!(*mock.item.borrow(), Some(vec![2]));
    assert!(remove_with(&generation, &mock).is_err());
    assert_eq!(mock.deletes.get(), 1);
}
#[test]
fn successful_removal_retains_private_identity_evidence() {
    let (_temp, generation, mock) = fixture();
    remove_with(&generation, &mock).unwrap();
    assert_eq!(mock.deletes.get(), 1);
    assert!(mock.item.borrow().is_none());
    let record: Removal =
        state::read_json(&generation.binding.directory.join("native-removal.json")).unwrap();
    assert_eq!(record.generation, generation.binding.id);
    assert_eq!(record.reference, vec![1]);
}

#[test]
fn retry_does_not_treat_changed_item_attributes_as_deleted_identity() {
    let (_temp, generation, mut mock) = fixture();
    mock.fail_after_delete.set(true);
    assert!(remove_with(&generation, &mock).is_err());
    *mock.item.borrow_mut() = Some(vec![1]);
    mock.account = "changed-account".into();
    assert!(remove_with(&generation, &mock).is_err());
    assert_eq!(mock.deletes.get(), 1);
    assert_eq!(*mock.item.borrow(), Some(vec![1]));
}
