// /api/v1/training as an Express router over the Training req.svc. The local
// app mounts it with a human session actor; the hosted app (M2) will mount the
// same contract with workspace authentication. Policy stays in the req.svc.
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

// `service` is a Training service, or `async (req) => service` for one per account.
// hosted: exports come back as a zip download (nothing is written on the server) and uploads
// are capped at uploadLimit.
export function trainingRouter(
  service,
  { actorFor, onChange = () => {}, hosted = false, uploadLimit = '512mb' },
) {
  const router = express.Router();
  const json = express.json({ limit: '1mb', strict: true });
  router.use(async (req, res, next) => {
    try {
      req.svc = typeof service === 'function' ? await service(req) : service;
      next();
    } catch (error) {
      res
        .status(error.status >= 400 ? error.status : 500)
        .json({ error: { code: error.code || 'REQUEST_FAILED', message: error.message } });
    }
  });
  const a = (req) => actorFor(req);
  const changed = (kind, req) => (result) => {
    onChange(kind, req);
    return result;
  };
  const page = (req) => Math.min(Number(req.query.limit) || 100, 1000);

  router.get(
    '/status',
    wrap((req) => req.svc.status()),
  );
  router.get(
    '/inbox',
    wrap((req) => req.svc.inbox({ all: req.query.all === '1' })),
  );

  router.get(
    '/imports',
    wrap((req) => req.svc.listImports()),
  );
  router.post(
    '/imports',
    express.raw({ type: () => true, limit: uploadLimit }),
    wrap(async (req) => {
      const meta = req.get('x-w3bs-import') ? JSON.parse(req.get('x-w3bs-import')) : {};
      return req.svc
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
        .then(changed('imports', req));
    }),
  );
  router.get(
    '/imports/:id',
    wrap((req) => req.svc.getImport(req.params.id)),
  );
  router.post(
    '/imports/:id/complete',
    wrap((req) => req.svc.commitImport(req.params.id, a(req)).then(changed('episodes', req))),
  );

  router.get(
    '/sessions',
    wrap((req) => req.svc.listEpisodes({ state: req.query.state, limit: page(req) })),
  );
  router.get(
    '/episodes',
    wrap((req) => req.svc.listEpisodes({ state: req.query.state, limit: page(req) })),
  );
  router.get(
    '/episodes/:id',
    wrap((req) => req.svc.getEpisode(req.params.id)),
  );
  router.patch(
    '/episodes/:id',
    json,
    wrap((req) => req.svc.review(req.params.id, req.body, a(req)).then(changed('episodes', req))),
  );
  router.post(
    '/episodes/:id/review',
    json,
    wrap((req) => req.svc.review(req.params.id, req.body, a(req)).then(changed('episodes', req))),
  );
  router.post(
    '/episodes/:id/split',
    json,
    wrap((req) => req.svc.split(req.params.id, req.body, a(req)).then(changed('episodes', req))),
  );
  router.post(
    '/episodes/:id/moshscript',
    wrap(async (req, res) => {
      const result = await req.svc.exportMoshscript(req.params.id, a(req));
      onChange('exports', req);
      res.set({
        'content-type': 'text/plain; charset=utf-8',
        'content-disposition': `attachment; filename="${result.filename.replace(/[^A-Za-z0-9._-]/g, '_')}"`,
        'x-w3bs-export': JSON.stringify({ export_id: result.export_id, run: result.run }),
      });
      res.end(result.script);
    }),
  );
  router.post(
    '/episodes/merge',
    json,
    wrap((req) => req.svc.merge(req.body.episodes, a(req)).then(changed('episodes', req))),
  );

  router.get(
    '/datasets',
    wrap((req) => req.svc.listDatasets()),
  );
  router.post(
    '/datasets',
    json,
    wrap((req) => req.svc.createDataset(req.body, a(req)).then(changed('datasets', req))),
  );
  router.get(
    '/datasets/:id',
    wrap((req) => req.svc.inspectDataset(req.params.id)),
  );
  router.patch(
    '/datasets/:id',
    json,
    wrap((req) =>
      req.svc.updateDataset(req.params.id, req.body, a(req)).then(changed('datasets', req)),
    ),
  );
  router.delete(
    '/datasets/:id',
    wrap((req) =>
      req.svc
        .deleteDataset(req.params.id, { confirm: req.query.confirm === '1' }, a(req))
        .then(changed('datasets', req)),
    ),
  );
  router.post(
    '/datasets/:id/episodes',
    json,
    wrap(async (req) => {
      if (req.body.add?.length) await req.svc.addEpisodes(req.params.id, req.body.add, a(req));
      if (req.body.remove?.length)
        await req.svc.removeEpisodes(req.params.id, req.body.remove, a(req));
      onChange('datasets', req);
      return req.svc.inspectDataset(req.params.id);
    }),
  );
  router.post(
    '/datasets/:id/versions',
    json,
    wrap((req) =>
      req.svc.createVersion(req.params.id, req.body ?? {}, a(req)).then(changed('datasets', req)),
    ),
  );
  router.get(
    '/datasets/:id/versions/:version',
    wrap((req) => req.svc.getVersion(req.params.id, req.params.version)),
  );
  router.post(
    '/datasets/:id/versions/:version/approve',
    json,
    wrap((req) =>
      req.svc
        .approve(req.params.id, req.params.version, req.body.purposes ?? [], a(req))
        .then(changed('datasets', req)),
    ),
  );
  router.post(
    '/datasets/:id/versions/:version/release',
    wrap((req) =>
      req.svc.release(req.params.id, req.params.version, a(req)).then(changed('datasets', req)),
    ),
  );
  router.post(
    '/datasets/:id/versions/:version/revoke',
    wrap((req) =>
      req.svc.revoke(req.params.id, req.params.version, a(req)).then(changed('datasets', req)),
    ),
  );

  router.get(
    '/grants',
    wrap((req) => req.svc.listGrants()),
  );
  router.post(
    '/grants',
    json,
    wrap((req) => req.svc.grant(req.body, a(req)).then(changed('grants', req))),
  );
  router.post(
    '/share',
    json,
    wrap((req) =>
      req.svc
        .shareReviewed(req.body, a(req))
        .then(changed('datasets', req))
        .then(changed('grants', req)),
    ),
  );
  router.delete(
    '/grants/:id',
    wrap((req) => req.svc.revokeGrant(req.params.id, a(req)).then(changed('grants', req))),
  );
  router.post(
    '/access',
    json,
    wrap((req) =>
      req.svc.setAgentAccess(req.body.on === true, a(req)).then(changed('grants', req)),
    ),
  );
  router.post(
    '/retrieve',
    json,
    wrap((req) => req.svc.retrieve(req.body, a(req))),
  );

  router.get(
    '/exports',
    wrap((req) =>
      req.svc.auditLog({ limit: 500 }).then((rows) => rows.filter((r) => r.operation === 'export')),
    ),
  );
  router.post(
    '/exports',
    json,
    wrap(async (req, res) => {
      const options = hosted
        ? { format: req.body.format, purpose: req.body.purpose, download: true }
        : req.body;
      const result = await req.svc.export(req.body.dataset_id, req.body.version, options, a(req));
      onChange('exports', req);
      if (!result.data) return result;
      res.set({
        'content-type': 'application/zip',
        'content-disposition': `attachment; filename="${result.filename.replace(/[^A-Za-z0-9._-]/g, '_')}"`,
        'x-w3bs-export': JSON.stringify({ export_id: result.export_id, excluded: result.excluded }),
      });
      res.end(result.data);
    }),
  );

  router.get(
    '/runs',
    wrap((req) => req.svc.listRuns({ consumer: req.query.consumer, limit: page(req) })),
  );
  router.post(
    '/runs/:id/review',
    json,
    wrap((req) => req.svc.reviewRun(req.params.id, req.body, a(req)).then(changed('runs', req))),
  );
  router.get(
    '/evaluations',
    wrap((req) =>
      req.svc.evaluate({ consumer: req.query.consumer, task_family: req.query.task_family }),
    ),
  );
  router.get(
    '/workflows',
    wrap((req) => req.svc.listWorkflows()),
  );
  router.post(
    '/workflows',
    json,
    wrap((req) =>
      req.svc.deriveWorkflow(req.body.episode_id, a(req)).then(changed('workflows', req)),
    ),
  );
  router.get(
    '/training-jobs',
    wrap(async (req) => ({ adapters: await req.svc.trainingAdapters(), jobs: [] })),
  );
  router.post(
    '/training-jobs',
    json,
    wrap((req) => req.svc.submitTrainingJob(req.body, a(req))),
  );
  router.get(
    '/approvals',
    wrap((req) => req.svc.listApprovals()),
  );
  router.post(
    '/approvals/:id',
    json,
    wrap((req) =>
      req.svc
        .decideApproval(req.params.id, req.body.approve === true, a(req))
        .then(changed('approvals', req)),
    ),
  );

  router.post(
    '/deletion-requests',
    json,
    wrap((req) =>
      (req.body.episodes
        ? req.svc.deleteEpisodes(req.body.episodes, a(req))
        : req.svc.forget({ from: req.body.from, to: req.body.to }, a(req))
      ).then(changed('episodes', req)),
    ),
  );
  router.get(
    '/deletion-requests/:id',
    wrap((req) => req.svc.getDeletion(req.params.id)),
  );
  router.get(
    '/audit',
    wrap((req) => req.svc.auditLog()),
  );

  // Artifact bytes need a current human session; there are no public URLs.
  router.get(
    '/artifacts/:id',
    wrap(async (req, res) => {
      const artifact = await req.svc.artifact(req.params.id, a(req));
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
