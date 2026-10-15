// /api/v1/training as an Express router over the Training service. The local
// app mounts it with a human session actor; the hosted app (M2) will mount the
// same contract with workspace authentication. Policy stays in the service.
import express from 'express';

const wrap = (fn) => async (req, res) => {
  try {
    const result = await fn(req, res);
    if (result !== undefined) res.json(result);
  } catch (error) {
    res.status(error.status && error.status >= 400 ? error.status : 500).json({
      error: {
        code: error.code || 'REQUEST_FAILED',
        message: error.message,
        ...(error.details ? { details: error.details } : {}),
      },
    });
  }
};

export function trainingRouter(service, { actorFor, onChange = () => {} }) {
  const router = express.Router();
  const json = express.json({ limit: '1mb', strict: true });
  const changed = (kind) => (result) => {
    onChange(kind);
    return result;
  };
  const a = (req) => actorFor(req);
  const page = (req) => Math.min(Number(req.query.limit) || 100, 1000);

  router.get(
    '/status',
    wrap(() => service.status()),
  );
  router.get(
    '/inbox',
    wrap((req) => service.inbox({ all: req.query.all === '1' })),
  );

  router.get(
    '/imports',
    wrap(() => service.listImports()),
  );
  router.post(
    '/imports',
    express.raw({ type: () => true, limit: '512mb' }),
    wrap(async (req) => {
      const meta = req.get('x-w3bs-import') ? JSON.parse(req.get('x-w3bs-import')) : {};
      return service
        .importFile(
          {
            data: req.body,
            filename: String(req.get('x-filename') || 'upload'),
            mapping: meta.mapping,
            annotations: meta.annotations,
            rights: meta.rights,
            allowText: meta.allow_text === true,
          },
          a(req),
        )
        .then(changed('imports'));
    }),
  );
  router.get(
    '/imports/:id',
    wrap((req) => service.getImport(req.params.id)),
  );
  router.post(
    '/imports/:id/complete',
    wrap((req) => service.commitImport(req.params.id, a(req)).then(changed('episodes'))),
  );

  router.get(
    '/sessions',
    wrap((req) => service.listEpisodes({ state: req.query.state, limit: page(req) })),
  );
  router.get(
    '/episodes',
    wrap((req) => service.listEpisodes({ state: req.query.state, limit: page(req) })),
  );
  router.get(
    '/episodes/:id',
    wrap((req) => service.getEpisode(req.params.id)),
  );
  router.patch(
    '/episodes/:id',
    json,
    wrap((req) => service.review(req.params.id, req.body, a(req)).then(changed('episodes'))),
  );
  router.post(
    '/episodes/:id/review',
    json,
    wrap((req) => service.review(req.params.id, req.body, a(req)).then(changed('episodes'))),
  );
  router.post(
    '/episodes/:id/split',
    json,
    wrap((req) => service.split(req.params.id, req.body, a(req)).then(changed('episodes'))),
  );
  router.post(
    '/episodes/merge',
    json,
    wrap((req) => service.merge(req.body.episodes, a(req)).then(changed('episodes'))),
  );

  router.get(
    '/datasets',
    wrap(() => service.listDatasets()),
  );
  router.post(
    '/datasets',
    json,
    wrap((req) => service.createDataset(req.body, a(req)).then(changed('datasets'))),
  );
  router.get(
    '/datasets/:id',
    wrap((req) => service.inspectDataset(req.params.id)),
  );
  router.patch(
    '/datasets/:id',
    json,
    wrap((req) => service.updateDataset(req.params.id, req.body, a(req)).then(changed('datasets'))),
  );
  router.delete(
    '/datasets/:id',
    wrap((req) =>
      service
        .deleteDataset(req.params.id, { confirm: req.query.confirm === '1' }, a(req))
        .then(changed('datasets')),
    ),
  );
  router.post(
    '/datasets/:id/episodes',
    json,
    wrap(async (req) => {
      if (req.body.add?.length) await service.addEpisodes(req.params.id, req.body.add, a(req));
      if (req.body.remove?.length)
        await service.removeEpisodes(req.params.id, req.body.remove, a(req));
      onChange('datasets');
      return service.inspectDataset(req.params.id);
    }),
  );
  router.post(
    '/datasets/:id/versions',
    json,
    wrap((req) =>
      service.createVersion(req.params.id, req.body ?? {}, a(req)).then(changed('datasets')),
    ),
  );
  router.get(
    '/datasets/:id/versions/:version',
    wrap((req) => service.getVersion(req.params.id, req.params.version)),
  );
  router.post(
    '/datasets/:id/versions/:version/approve',
    json,
    wrap((req) =>
      service
        .approve(req.params.id, req.params.version, req.body.purposes ?? [], a(req))
        .then(changed('datasets')),
    ),
  );
  router.post(
    '/datasets/:id/versions/:version/release',
    wrap((req) =>
      service.release(req.params.id, req.params.version, a(req)).then(changed('datasets')),
    ),
  );
  router.post(
    '/datasets/:id/versions/:version/revoke',
    wrap((req) =>
      service.revoke(req.params.id, req.params.version, a(req)).then(changed('datasets')),
    ),
  );

  router.get(
    '/grants',
    wrap(() => service.listGrants()),
  );
  router.post(
    '/grants',
    json,
    wrap((req) => service.grant(req.body, a(req)).then(changed('grants'))),
  );
  router.delete(
    '/grants/:id',
    wrap((req) => service.revokeGrant(req.params.id, a(req)).then(changed('grants'))),
  );
  router.post(
    '/access',
    json,
    wrap((req) => service.setAgentAccess(req.body.on === true, a(req)).then(changed('grants'))),
  );
  router.post(
    '/retrieve',
    json,
    wrap((req) => service.retrieve(req.body, a(req))),
  );

  router.get(
    '/exports',
    wrap(() =>
      service.auditLog({ limit: 500 }).then((rows) => rows.filter((r) => r.operation === 'export')),
    ),
  );
  router.post(
    '/exports',
    json,
    wrap((req) =>
      service
        .export(req.body.dataset_id, req.body.version, req.body, a(req))
        .then(changed('exports')),
    ),
  );

  router.get(
    '/runs',
    wrap((req) => service.listRuns({ consumer: req.query.consumer, limit: page(req) })),
  );
  router.post(
    '/runs/:id/review',
    json,
    wrap((req) => service.reviewRun(req.params.id, req.body, a(req)).then(changed('runs'))),
  );
  router.get(
    '/evaluations',
    wrap((req) =>
      service.evaluate({ consumer: req.query.consumer, task_family: req.query.task_family }),
    ),
  );
  router.get(
    '/workflows',
    wrap(() => service.listWorkflows()),
  );
  router.post(
    '/workflows',
    json,
    wrap((req) => service.deriveWorkflow(req.body.episode_id, a(req)).then(changed('workflows'))),
  );
  router.get(
    '/training-jobs',
    wrap(async () => ({ adapters: await service.trainingAdapters(), jobs: [] })),
  );
  router.post(
    '/training-jobs',
    json,
    wrap((req) => service.submitTrainingJob(req.body, a(req))),
  );
  router.get(
    '/approvals',
    wrap(() => service.listApprovals()),
  );
  router.post(
    '/approvals/:id',
    json,
    wrap((req) =>
      service
        .decideApproval(req.params.id, req.body.approve === true, a(req))
        .then(changed('approvals')),
    ),
  );

  router.post(
    '/deletion-requests',
    json,
    wrap((req) =>
      (req.body.episodes
        ? service.deleteEpisodes(req.body.episodes, a(req))
        : service.forget({ from: req.body.from, to: req.body.to }, a(req))
      ).then(changed('episodes')),
    ),
  );
  router.get(
    '/deletion-requests/:id',
    wrap((req) => service.getDeletion(req.params.id)),
  );
  router.get(
    '/audit',
    wrap(() => service.auditLog()),
  );

  // Artifact bytes need a current human session; there are no public URLs.
  router.get(
    '/artifacts/:id',
    wrap(async (req, res) => {
      const artifact = await service.artifact(req.params.id, a(req));
      res.set({
        'content-type': artifact.media_type,
        'cache-control': 'no-store',
        'x-content-type-options': 'nosniff',
        'content-disposition': 'inline',
      });
      res.end(artifact.data);
    }),
  );
  return router;
}
