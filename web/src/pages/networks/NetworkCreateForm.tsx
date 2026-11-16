import { useQuery } from '@tanstack/react-query'
import { useMemo, useState } from 'react'

import { fieldError, formError, formErrorDetail, formFailure } from '@/api/formErrors'
import { api } from '@/api/endpoints'
import { q, useAction } from '@/api/queries'
import { type Network } from '@/api/types'
import { Notice } from '@/components/notice'
import { Field } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { t } from '@/i18n'
import { cidrContains, cidrOverlaps, parseCidr, SHARED_ADDRESS_SPACE, suggestCidr } from '@/lib/cidr'
import { validateName, validateNetworkCidr } from '@/lib/validate'

/** 地址段和已有的网络重叠时，说出是和哪一个。 */
function overlapMessage(raw: string, networks: readonly Network[]): string | null {
  const cidr = parseCidr(raw)
  if (cidr === null) return null
  for (const other of networks) {
    const otherCidr = parseCidr(other.cidr)
    if (otherCidr !== null && cidrOverlaps(cidr, otherCidr)) {
      return t.networks.cidrOverlaps(other.name, other.cidr)
    }
  }
  return null
}

/**
 * 新建网络的表单的状态和提交。首次进入的那一屏和"新建网络"对话框用的是同一个表单，只是外面的壳不一样。
 */
export function useNetworkCreateForm(onCreated: (network: Network) => void) {
  const networks = useQuery(q.networks())
  const existing = useMemo(() => networks.data ?? [], [networks.data])

  const defaultName = existing.some((network) => network.name === 'home') ? '' : 'home'
  const suggested = useMemo(() => suggestCidr(existing.map((network) => network.cidr)), [existing])

  const [name, setName] = useState<string | null>(null)
  const [cidr, setCidr] = useState<string | null>(null)
  const [touched, setTouched] = useState(false)
  const nameValue = name ?? defaultName
  const cidrValue = cidr ?? suggested

  const create = useAction(null, {
    mutationFn: () => api.createNetwork({ name: nameValue.trim().toLowerCase(), cidr: cidrValue.trim() }),
    success: (network) => t.networks.created(network.name),
    toastError: false,
  })

  const failure = formFailure(create.error, { name_taken: 'name', range_overlaps: 'cidr' } as const)
  const nameProblem = validateName(nameValue)
  const cidrProblem = validateNetworkCidr(cidrValue) ?? overlapMessage(cidrValue, existing)
  const parsed = parseCidr(cidrValue)
  const shared =
    parsed !== null && (cidrContains(SHARED_ADDRESS_SPACE, parsed) || cidrContains(parsed, SHARED_ADDRESS_SPACE))

  const submit = () => {
    setTouched(true)
    if (nameProblem !== null || cidrProblem !== null) return
    create.mutate(undefined, { onSuccess: onCreated })
  }

  const fields = (
    <>
      <Field
        label={t.networks.nameLabel}
        hint={t.networks.nameHint(
          nameValue.trim() === '' ? t.networks.namePlaceholder : nameValue.trim().toLowerCase(),
        )}
        error={(touched || name !== null ? nameProblem : null) ?? fieldError(failure, 'name')}
      >
        {(props) => (
          <Input
            {...props}
            name="name"
            value={nameValue}
            placeholder={t.networks.namePlaceholder}
            autoFocus
            onChange={(event) => {
              setName(event.target.value)
              create.reset()
            }}
          />
        )}
      </Field>
      <Field
        label={t.networks.cidrLabel}
        hint={t.networks.cidrHint}
        error={(touched || cidr !== null ? cidrProblem : null) ?? fieldError(failure, 'cidr')}
      >
        {(props) => (
          <Input
            {...props}
            name="cidr"
            className="font-mono"
            value={cidrValue}
            onChange={(event) => {
              setCidr(event.target.value)
              create.reset()
            }}
          />
        )}
      </Field>
      {shared && cidrProblem === null ? <Notice tone="warn">{t.networks.cidrShared}</Notice> : null}
    </>
  )

  return {
    fields,
    submit,
    pending: create.isPending,
    error: formError(failure),
    errorDetail: formErrorDetail(failure),
    dirty: name !== null || cidr !== null,
    reset: () => {
      setName(null)
      setCidr(null)
      setTouched(false)
      create.reset()
    },
  }
}
