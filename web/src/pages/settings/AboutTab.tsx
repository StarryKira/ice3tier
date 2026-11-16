import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router'

import { METRICS_PATH } from '@/api/client'
import { q } from '@/api/queries'
import { CopyText } from '@/components/copy'
import { InfoRow } from '@/components/page'
import { ErrorState, SkeletonBlock } from '@/components/states'
import { StatusText } from '@/components/status'
import { Card, CardBody, CardHeader } from '@/components/ui/card'
import { t } from '@/i18n'
import { WEB_VERSION } from '@/lib/version'

const s = t.settings
const v = t.version

/** 关于：两边的版本、controller 的身份、指标的地址、数据目录（备份就是复制它）。 */
export function AboutTab() {
  const status = useQuery(q.status())
  return (
    <Card>
      <CardHeader title={s.about.title} />
      {status.isError ? (
        <ErrorState title={s.about.loadFailed} error={status.error} onRetry={() => void status.refetch()} />
      ) : status.data === undefined ? (
        <SkeletonBlock lines={5} />
      ) : (
        <CardBody>
          <dl className="space-y-2">
            <InfoRow label={v.aboutController}>
              <span className="tabular" data-testid="about-controller-version">
                {status.data.version}
              </span>
              {status.data.newer_version !== null ? (
                <span className="mt-0.5 block text-[13px]">
                  <StatusText tone="warn">{v.newer(status.data.newer_version)}</StatusText>
                  <span className="block text-muted-foreground">{v.newerHint(status.data.newer_version)}</span>
                  <Link to="/settings/updates" className="text-primary hover:underline">
                    {v.newerLink}
                  </Link>
                </span>
              ) : null}
            </InfoRow>
            <InfoRow label={v.aboutWeb}>
              <span className="tabular" data-testid="about-web-version">
                {WEB_VERSION}
              </span>
              {status.data.version === WEB_VERSION ? null : (
                <span className="mt-0.5 block text-[13px] text-warn" data-testid="about-version-mismatch">
                  {v.mismatch(WEB_VERSION, status.data.version)}
                </span>
              )}
            </InfoRow>
            <InfoRow label={s.about.publicAddr}>
              <CopyText value={status.data.public_addr} />
              <span className="mt-0.5 block text-[13px] text-muted-foreground">{s.about.publicAddrHint}</span>
            </InfoRow>
            <InfoRow label={s.about.key}>
              <CopyText value={status.data.controller_key} wrap />
            </InfoRow>
            <InfoRow label={s.about.totals}>
              <span className="tabular">
                {s.about.totalsValue(status.data.networks, status.data.nodes, status.data.online)}
              </span>
            </InfoRow>
            <InfoRow label={v.metrics}>
              <CopyText value={`${window.location.origin}${METRICS_PATH}`} />
              <span className="mt-0.5 block text-[13px] text-muted-foreground">{v.metricsHint}</span>
            </InfoRow>
            <InfoRow label={s.about.dataDir}>
              {status.data.data_dir === null ? (
                <span className="text-muted-foreground">{s.about.dataDirNone}</span>
              ) : (
                <>
                  <CopyText value={status.data.data_dir} wrap />
                  <span className="mt-0.5 block text-[13px] text-muted-foreground">{s.about.dataDirHint}</span>
                </>
              )}
            </InfoRow>
            <InfoRow label={s.about.offline}>
              <span className="text-[13px] text-muted-foreground">{s.about.offlineBody}</span>
            </InfoRow>
          </dl>
        </CardBody>
      )}
    </Card>
  )
}
