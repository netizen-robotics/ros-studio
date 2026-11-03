use super::*;
use crate::robot::{LoadOptions, MeshAsset, MeshAssetLoader, RobotError, RobotErrorKind};
struct Cache {
    policy: Arc<AssetPolicy>,
    loader: Option<Result<MeshAssetLoader, RobotError>>,
    resources: BTreeMap<(Arc<str>, bool), Result<Arc<MeshAsset>, RobotError>>,
}
impl Cache {
    fn load(
        &mut self,
        resource: &Arc<str>,
        embedded: bool,
        cancel: &AtomicBool,
    ) -> Result<Arc<MeshAsset>, RobotError> {
        let key = (resource.clone(), embedded);
        if let Some(result) = self.resources.get(&key) {
            return result.clone();
        }
        if self.resources.len() >= crate::robot::MAX_MESH_RESOURCES {
            return Err(RobotError::new(
                RobotErrorKind::Limit,
                "Marker resource cache full; Reload explicitly",
            ));
        }
        let loader = self.loader.get_or_insert_with(|| {
            MeshAssetLoader::new(
                self.policy.root.as_deref(),
                &LoadOptions {
                    package_roots: self.policy.packages.clone(),
                },
                cancel,
            )
        });
        let result = match loader {
            Ok(loader) => loader.load(resource, embedded, cancel),
            Err(error) => Err(error.clone()),
        };
        if self.loader.as_ref().is_some_and(
            |loader| matches!(loader, Err(error) if error.kind == RobotErrorKind::Cancelled),
        ) {
            self.loader = None;
        }
        if !matches!(&result, Err(error) if error.kind == RobotErrorKind::Cancelled) {
            self.resources.insert(key, result.clone());
        }
        result
    }
}
pub(super) struct Preparation {
    clock: MarkerClock,
    assets: Option<Cache>,
    last_asset_epoch: u64,
}
impl Preparation {
    pub fn new(clock: MarkerClock) -> Self {
        Self {
            clock,
            assets: None,
            last_asset_epoch: 0,
        }
    }
    fn policy(&mut self, policy: Option<Arc<AssetPolicy>>) -> Result<(), String> {
        match policy {
            None => self.assets = None,
            Some(policy) => {
                if let Some(cache) = &self.assets {
                    if cache.policy.epoch == policy.epoch {
                        return if cache.policy == policy {
                            Ok(())
                        } else {
                            Err("Marker asset policy changed without a new epoch".into())
                        };
                    }
                }
                if policy.epoch <= self.last_asset_epoch {
                    return Err(
                        "Marker asset policy epoch was retired; use a fresh Reload epoch".into(),
                    );
                }
                self.last_asset_epoch = policy.epoch;
                self.assets = Some(Cache {
                    policy,
                    loader: None,
                    resources: BTreeMap::new(),
                });
            }
        }
        Ok(())
    }
    pub fn run<C>(
        &mut self,
        request: PrepareRequest<C>,
        cancel: &AtomicBool,
    ) -> Result<Arc<PreparedScene<C>>, String> {
        let began = Instant::now();
        cancelled(cancel)?;
        request.validate()?;
        self.policy(request.assets.clone())?;
        let mut raw = vec![];
        let mut diagnostics = vec![];
        let mut owners = vec![];
        for owner in request.owners {
            cancelled(cancel)?;
            let now = owner
                .origin
                .elapsed()
                .as_nanos()
                .try_into()
                .unwrap_or(u64::MAX);
            let prepared =
                super::super::marker_geometry::prepare(owner.state.entries(), now, cancel)?;
            diagnostics.extend(prepared.hidden.into_iter().map(|h| Diagnostic {
                owner: owner.owner,
                key: h.key,
                message: format!("{:?}", h.status),
            }));
            for entry in prepared.entries {
                let mesh = matches!(entry.body, Body::Mesh { .. });
                let deadline = entry
                    .expires_at_monotonic_ns
                    .map(|d| self.clock.deadline_from(owner.origin, d))
                    .transpose()?;
                raw.push((
                    KeyIdentity {
                        owner: owner.owner,
                        source_epoch: owner.source_epoch,
                        scene_epoch: request.scene_epoch,
                        asset_epoch: if mesh {
                            request.assets.as_ref().map(|p| p.epoch)
                        } else {
                            None
                        },
                        key: entry.key,
                        version: entry.version,
                    },
                    deadline,
                    entry.body,
                ));
            }
            owners.push(OwnerCapture {
                owner: owner.owner,
                source_epoch: owner.source_epoch,
                revision: owner.revision,
                wire_owner: owner.state.header().owner.clone(),
                context: owner.context,
            });
        }
        cancelled(cancel)?;
        let text_indices: Vec<_> = raw
            .iter()
            .enumerate()
            .filter_map(|(i, (_, _, body))| matches!(body, Body::Text { .. }).then_some(i))
            .collect();
        let bodies: Vec<_> = text_indices.iter().map(|&i| &raw[i].2).collect();
        let layouts = if bodies.is_empty() {
            vec![]
        } else {
            marker_text::prepare(&bodies, || cancel.load(Ordering::Acquire))?
        };
        let mut drawing: BTreeMap<usize, Drawing> = text_indices
            .into_iter()
            .zip(layouts)
            .map(|(i, layout)| (i, Drawing::Text(layout)))
            .collect();
        let mut loaded = vec![];
        for (i, (identity, _, body)) in raw.iter().enumerate() {
            cancelled(cancel)?;
            if let Body::Mesh {
                resource, embedded, ..
            } = body
            {
                let result = match &mut self.assets {
                    Some(cache) => cache.load(resource, *embedded, cancel),
                    None => Err(RobotError::new(
                        RobotErrorKind::Asset,
                        "Review a local Marker mesh asset root or package mapping",
                    )),
                };
                match result {
                    Ok(asset) => loaded.push((i, asset)),
                    Err(error)
                        if matches!(
                            error.kind,
                            RobotErrorKind::Limit | RobotErrorKind::Cancelled
                        ) =>
                    {
                        return Err(error.to_string())
                    }
                    Err(error) => diagnostics.push(Diagnostic {
                        owner: identity.owner,
                        key: identity.key.clone(),
                        message: error.to_string(),
                    }),
                }
            }
        }
        let inputs: Vec<_> = loaded
            .iter()
            .map(|(i, asset)| (&raw[*i].2, asset.as_ref()))
            .collect();
        let meshes = marker_mesh::prepare(&inputs, cancel)?;
        for ((i, _), mesh) in loaded.into_iter().zip(meshes) {
            match mesh {
                Ok(mesh) => {
                    drawing.insert(i, Drawing::Mesh(mesh));
                }
                Err(message) => diagnostics.push(Diagnostic {
                    owner: raw[i].0.owner,
                    key: raw[i].0.key.clone(),
                    message,
                }),
            }
        }
        let mut items = vec![];
        for (i, (identity, deadline, body)) in raw.into_iter().enumerate() {
            cancelled(cancel)?;
            let prepared = match body {
                Body::Text { .. } | Body::Mesh { .. } => drawing.remove(&i),
                _ => Some(Drawing::Geometry(body)),
            };
            if let Some(drawing) = prepared {
                let bounds = match super::bounds::prepare(&drawing) {
                    Ok(bounds) => bounds,
                    Err(message) => {
                        diagnostics.push(Diagnostic {
                            owner: identity.owner,
                            key: identity.key.clone(),
                            message,
                        });
                        continue;
                    }
                };
                items.push(Item {
                    identity,
                    deadline,
                    drawing,
                    bounds,
                });
            }
        }
        Ok(Arc::new(PreparedScene {
            items,
            clock: self.clock.clone(),
            scene_epoch: request.scene_epoch,
            owners,
            diagnostics,
            preparation_time: began.elapsed(),
        }))
    }
}
