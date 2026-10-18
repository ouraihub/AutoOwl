import type { RenderTask, SourceId } from '$lib/domain/contracts';
import { createTauriClient } from '$lib/services/tauri-client';

const client = createTauriClient();

export const renderService = {
  /** Starts an ffmpeg render of cover + audio into a video file. Requires ffmpeg. */
  startVideoRender(
    songId: string,
    source: SourceId,
    coverPath: string,
    audioPath: string
  ): Promise<RenderTask> {
    return client.invoke<RenderTask>('start_video_render', {
      songId,
      source,
      coverPath,
      audioPath
    });
  }
};
