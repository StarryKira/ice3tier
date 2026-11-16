import { useNavigate } from 'react-router'

import { FormDialog } from '@/components/form-dialog'
import { t } from '@/i18n'

import { networkPath } from './context'
import { useNetworkCreateForm } from './NetworkCreateForm'

interface CreateNetworkDialogProps {
  open: boolean
  onClose: () => void
}

function CreateNetworkForm({ onClose }: { onClose: () => void }) {
  const navigate = useNavigate()
  // 创建后自动切换到新网络，落在总览页，也就是它的引导。
  const form = useNetworkCreateForm((network) => {
    onClose()
    void navigate(networkPath(network.id))
  })

  return (
    <FormDialog
      open
      onClose={onClose}
      title={t.networks.createTitle}
      dirty={form.dirty}
      submitLabel={t.networks.createSubmit}
      pending={form.pending}
      error={form.error}
      errorDetail={form.errorDetail}
      onSubmit={form.submit}
    >
      {form.fields}
      <ul className="space-y-1 text-[13px] text-muted-foreground">
        <li>{t.networks.noteDefaultRule}</li>
        <li>{t.networks.noteIsolation}</li>
      </ul>
    </FormDialog>
  )
}

/** 新建网络。每次打开都是一张新的表单。 */
export function CreateNetworkDialog({ open, onClose }: CreateNetworkDialogProps) {
  return open ? <CreateNetworkForm onClose={onClose} /> : null
}
