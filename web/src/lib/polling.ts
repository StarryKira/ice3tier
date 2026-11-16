import { REPORT_INTERVAL_SECS } from './product'

/**
 * 管理 API 没有推送，要实时数据只能轮询。
 *
 * 标签页不在前台时不轮询：TanStack Query 的 `refetchInterval` 默认就在页面不可见时停下
 * （`refetchIntervalInBackground` 是 false），回到前台时立刻补取一次（`refetchOnWindowFocus`）。
 */

/** 显示在线状态的列表：和节点上报的周期一样，更勤没有意义。 */
export const LIVE_INTERVAL = REPORT_INTERVAL_SECS * 1000

/** 正等着某件事发生（节点用令牌加入、节点上报新的声明）：取得勤一些，用户这时正盯着页面。 */
export const WAITING_INTERVAL = 3_000

/** 总览页的引导：节点不足两个时一直在等新节点出现，不管它是从这个页面加的还是用命令行加的。 */
export const GUIDE_INTERVAL = 5_000
