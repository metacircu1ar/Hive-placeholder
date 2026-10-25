use super::{Result, Values, removal};
impl Values {
    pub(super) fn validate(&self) -> Result<()> {
        if self.purge == Some(true) && self.uninstall != Some(true) {
            return Err(removal::invalid(
                "--purge",
                "--purge requires --uninstall.",
                "use --uninstall --purge",
            ));
        }
        if self.purge_comb_root.is_some() && self.purge != Some(true) {
            return Err(removal::invalid(
                "--purge-comb-root",
                "--purge-comb-root requires --purge.",
                "use --uninstall --purge, or omit this scope option",
            ));
        }
        if self.prefix.is_some() && self.uninstall != Some(true) {
            return Err(removal::invalid(
                "--prefix",
                "--prefix applies to --uninstall.",
                "use --uninstall, or omit --prefix",
            ));
        }
        if self.uninstall == Some(true) {
            return removal::values(self);
        }
        if self
            .services
            .zip(self.no_services)
            .is_some_and(|(yes, no)| yes == no)
        {
            return Err(removal::invalid(
                "--services",
                "Conflicting service switches.",
                "choose --services or --no-services, or supply opposite boolean values",
            ));
        }
        let positive_comb = self.positive_comb();
        if positive_comb && (!cfg!(feature = "comb") || self.no_comb == Some(true)) {
            return Err(removal::invalid(
                self.comb_field(),
                "Comb setup is disabled.",
                "use an Comb-capable Hive build without --no-comb, or omit Comb setup options",
            ));
        }
        if self.no_services == Some(true)
            && (self.service_environment.is_some() || self.service_path.is_some())
        {
            return Err(removal::invalid(
                if self.service_environment.is_some() {
                    "--service-environment"
                } else {
                    "--service-path"
                },
                "Service settings conflict with --no-services.",
                "omit --no-services, or omit the service settings",
            ));
        }
        Ok(())
    }

    fn comb_field(&self) -> &'static str {
        [
            ("--comb-root", self.comb_root.is_some()),
            ("--embedding-config", self.embedding_config.is_some()),
            ("--bridge-file", self.bridge_file.is_some()),
            ("--collectors", self.collectors == Some(true)),
            ("--no-comb-roles", self.no_comb_roles == Some(false)),
        ]
        .into_iter()
        .find_map(|(field, present)| present.then_some(field))
        .unwrap_or("arguments")
    }
}
