import React, {useEffect, useRef} from 'react';
import useBaseUrl from '@docusaurus/useBaseUrl';
import 'asciinema-player/dist/bundle/asciinema-player.css';

export default function AsciinemaRecording({src, label, poster}) {
  const recordingUrl = useBaseUrl(src);
  const container = useRef(null);

  useEffect(() => {
    let disposed = false;
    let player;

    import('asciinema-player').then(({create}) => {
      if (!disposed && container.current) {
        player = create(recordingUrl, container.current, {
          cols: 178,
          rows: 40,
          controls: true,
          idleTimeLimit: 2,
          poster,
        });
      }
    });

    return () => {
      disposed = true;
      player?.dispose();
    };
  }, [recordingUrl, poster]);

  return (
    <div className="pi-recording" role="group" aria-label={label}>
      <div ref={container} />
    </div>
  );
}
