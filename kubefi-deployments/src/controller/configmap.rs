use std::rc::Rc;

use anyhow::Result;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::api::DeleteParams;
use kube::Client;

use crate::controller::{create_from_yaml, from_yaml, get_api, get_or_create};
use crate::crd::{AuthLdap, NiFiDeployment};
use crate::template::Template;

use super::either::Either::{Left, Right};

pub struct ConfigMapController {
    pub client: Rc<Client>,
    pub template: Rc<Template>,
}

impl ConfigMapController {
    pub async fn handle_configmaps(
        &self,
        d: &NiFiDeployment,
        name: &str,
        ns: &str,
    ) -> Result<bool> {
        let zk_cm_name = format!("{}-zookeeper", &name);
        let zk_cm = get_or_create::<ConfigMap, _>(&self.client, &zk_cm_name, &name, &ns, |name| {
            self.template.zk_configmap(name)
        });

        let nifi_cm_name = format!("{}-config", &name);
        let nifi_cm =
            get_or_create::<ConfigMap, _>(&self.client, &nifi_cm_name, &name, &ns, |name| {
                self.template.nifi_configmap(name, &d.spec.ldap)
            });

        let (r1, r2) = futures::future::join(zk_cm, nifi_cm).await;
        let nifi_cm = r1.and(r2)?;

        match nifi_cm {
            Left(maybe_cm) => match maybe_cm {
                Some(cm) => self.handle_update(&d, &name, &ns, &nifi_cm_name, cm).await,
                None => Ok(false),
            },
            Right(_) => Ok(false),
        }
    }

    async fn handle_update(
        &self,
        d: &NiFiDeployment,
        name: &str,
        ns: &str,
        cm_name: &str,
        current: ConfigMap,
    ) -> Result<bool> {
        let ldap = &d.spec.ldap;
        let maybe_yaml = self.template.nifi_configmap(&cm_name, ldap)?;
        match maybe_yaml {
            Some(yaml) => {
                let expected_cm = from_yaml::<ConfigMap>(&yaml)?;
                let expected_data = expected_cm.data;
                if current.data != expected_data {
                    self.recreate_cm(&name, &ns, &cm_name, ldap)
                        .await
                        .map(|_| true)
                } else {
                    Ok(false)
                }
            }
            None => Ok(false),
        }
    }

    async fn recreate_cm(
        &self,
        name: &str,
        ns: &str,
        nifi_cm_name: &str,
        ldap: &Option<AuthLdap>,
    ) -> Result<()> {
        let params = &DeleteParams::default();
        let api = get_api::<ConfigMap>(&self.client, &ns);
        api.delete(&nifi_cm_name, params).await?;

        debug!("Creating new ConfigMap: {}", &nifi_cm_name);
        create_from_yaml::<ConfigMap, _>(&name, &ns, &self.client, |name| {
            self.template.nifi_configmap(name, &ldap)
        })
        .await
        .map(|_| ())
    }
}
