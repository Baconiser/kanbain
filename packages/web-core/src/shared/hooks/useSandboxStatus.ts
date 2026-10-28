import { useQuery } from '@tanstack/react-query';
import { sandboxApi } from '@/shared/lib/api';

export const sandboxStatusKey = ['sandbox', 'status'] as const;

/** Docker sandbox status; polls faster while the image is building. */
export function useSandboxStatus() {
  return useQuery({
    queryKey: sandboxStatusKey,
    queryFn: () => sandboxApi.getStatus(),
    refetchInterval: (query) =>
      query.state.data?.image_building ? 5_000 : 60_000,
  });
}
